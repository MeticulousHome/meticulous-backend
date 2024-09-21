import asyncio
from asyncio import QueueEmpty
from gi.repository import Gio, GLib
import logging
import traceback
from threading import Thread

from log import MeticulousLogger

from named_thread import NamedThread

def run_glib_loop():
    loop = GLib.MainLoop()
    loop.run()

class DBUSException(Exception):
    pass

class AsyncDBUSClient(object):
    def __init__(self):
        self.logger = MeticulousLogger.getLogger(__name__)
        self.dbus_events = asyncio.Queue()
        self.loop = asyncio.new_event_loop()
        self.signal_subscriptions = []
        self.signal_callbacks = {}
        self.property_callbacks = {}

        self.system_bus = Gio.bus_get_sync(Gio.BusType.SYSTEM, None)
        self.logger.info(f"system bus is: [{self.system_bus}]")

        self.sys_bus_flags = self.system_bus.get_flags()
        self.logger.info(f"bus flags: [{self.sys_bus_flags}]")

        self.logger.info(f"bus dict representation: [{self.system_bus.__dict__}]")
        self.new_signal_subscription('org.freedesktop.DBus.Properties',
                                     'PropertiesChanged',
                                     self.property_changed_callback)
        
        self.new_property_subscription('de.pengutronix.rauc.Installer','Progress',self.just_print)

        self.thread = NamedThread(name="AsyncDbus thread", target=self.run_loop)
        self.thread.start()

        self.logger.info("AsyncDBus client initialized")
        
    def run_loop(self):
        try:
            self.logger.info(" dbus thread loop SET")
            asyncio.set_event_loop(self.loop)
            self.logger.info(" dbus thread loop SET")
            self.loop.create_task(asyncio.to_thread(run_glib_loop))
            self.loop.create_task(self.handle_dbus_event())
            self.logger.info(" dbus thread handler task created")
            self.loop.run_forever()
            self.logger.info("do we even reach this")
        except Exception as e:
            self.logger.error("Dbus monitor loop failed")
        
    def __del__(self):
        self.cleanup_dbus()

    def cleanup_dbus(self):
        """Unsubscribe on deletion."""
        for subscription in self.signal_subscriptions:
            self.system_bus.signal_unsubscribe(subscription)

        self.dbus_event_task.cancel()

    def on_dbus_event(self, *args):
        """Generic sync callback for all DBUS events."""
        self.dbus_events.put_nowait(args)
        self.logger.info(f"unread events on queue [{self.dbus_events.qsize()}]")


    async def handle_dbus_event(self):
        """
        Retrieves DBUS events from queue and calls corresponding async
        callback.
        """
        self.logger.info("handling dbus events")
        while True:
            try:
                event = self.dbus_events.get_nowait()
                interface = event[3]
                signal = event[4]

                await self.signal_callbacks[(interface, signal)](*event)
                
            except QueueEmpty as e:
                await asyncio.sleep(0.5)
                pass
            except Exception as e:
                self.logger.info(f"handling dbus events exception: [{e}]")
                traceback.print_exc()
                self.logger.error(str(e))


    def new_proxy(self, interface, object_path):
        """Returns a new managed proxy."""
        # assume name is interface without last part
        name = '.'.join(interface.split('.')[:-1])
        proxy = Gio.DBusProxy.new_sync(self.system_bus, 0, None, name,
                                       object_path, interface, None)

        # FIXME: check for methods
        if len(proxy.get_cached_property_names()) == 0:
            self.logger.warning('Proxy {} contains no properties')

        return proxy

    def new_signal_subscription(self, interface, signal, callback):
        """Add new signal subscription."""
        signal_subscription = self.system_bus.signal_subscribe(
            None, interface, signal, None, None, 0, self.on_dbus_event)
        self.logger.info(f"subscriptionId: [{signal_subscription}]")
        self.signal_callbacks[(interface, signal)] = callback
        self.signal_subscriptions.append(signal_subscription)

    def new_property_subscription(self, interface, property_, callback):
        """Add new property subscription."""
        self.property_callbacks[(interface, property_)] = callback

    async def property_changed_callback(self, connection, sender_name,
                                        object_path, interface_name,
                                        signal_name, parameters):
        """
        Callback for changed properties. Calls callbacks for changed
        properties as if they were signals.
        """
        property_interface = parameters[0]

        changed_properties = {k: v for k, v in parameters[1].items()
                              if (property_interface, k) in
                              self.property_callbacks}

        for attribute, status in changed_properties.items():
            await self.property_callbacks[(property_interface, attribute)](
                connection, sender_name, object_path, property_interface,
                attribute, status)
            

    async def just_print(self,  connection, sender_name, object_path, property_interface, attribute, status):
        self.logger.info(f"property: [{attribute}], is [{status}]")
        