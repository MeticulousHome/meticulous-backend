from typing import Any, Dict, Union, Optional
import sys
import asyncio
import os
from threading import Thread

from improv import *
from bless import (  # type: ignore
    BlessServer,
    BlessGATTCharacteristic,
    GATTCharacteristicProperties,
    GATTAttributePermissions
)

from wifi import WifiManager

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)

# NOTE: Some systems require different synchronization methods.
if sys.platform in ["darwin", "win32"]:
    raise ValueError("Cannot run on non-linux platforms")


class GATTServer():
    GATT_NAME = os.getenv('GATT_NAME', 'MeticulousEspresso')

    _singletonServer = None

    def __init__(self):
        logger.info("BLE init called")
        self.trigger = asyncio.Event()
        self.loop = asyncio.new_event_loop()

        def _exception_handler(loop, context):
            logger.exception("GATT Server crashed!",
                             exc_info=context, stack_info=True)
            GATTServer.getServer().stop()
        self.loop.set_exception_handler(_exception_handler)
        self.improv_server = ImprovProtocol(
            requires_authorization=False,
            wifi_connect_callback=GATTServer.wifi_connect,
            wifi_networks_callback=GATTServer.get_wifi_networks,
            max_response_bytes=250
        )
        self.bless_gatt_server = BlessServer(
            name=GATTServer.GATT_NAME, loop=self.loop)
        self.bless_gatt_server.read_request_func = GATTServer.read_request
        self.bless_gatt_server.write_request_func = GATTServer.write_request
        self.loopThread = None

    def getServer():
        if GATTServer._singletonServer is None:
            GATTServer._singletonServer = GATTServer()
        return GATTServer._singletonServer

    def allow_wifi_provisioning():
        server = GATTServer.getServer().improv_server
        if server.state == ImprovState.AWAITING_AUTHORIZATION:
            server.state = ImprovState.AUTHORIZED

    def is_running(self):
        if self.loopThread is None:
            return False

        return self.loopThread.is_alive()

    def start(self):
        logger.info(f"Starting BLE GATT Server")

        if not self.is_running():
            self.trigger.clear()

            def run_loop(loop):
                asyncio.set_event_loop(loop)
                loop.run_until_complete(self._ble_gatt_server_loop())

            self.loopThread = Thread(target=run_loop, args=(self.loop,))
            self.loopThread.start()

        else:
            logger.info("BLE GATT Server is already running")

    def stop(self):
        logger.info("Stopping BLE GATT Server")
        self.loop.call_soon_threadsafe(self.trigger.set)

    async def _ble_gatt_server_loop(self):

        # Power on the hci device if it is powered off
        await self.bless_gatt_server.setup_task
        interface = self.bless_gatt_server.adapter.get_interface(
            'org.bluez.Adapter1')
        powered = await interface.get_powered()
        if not powered:
            logger.info("bluetooth device is not powered, powering now!")
            await interface.set_powered(True)

        # Init and start GATT
        await self.bless_gatt_server.add_gatt(GATTServer._build_gatt())
        success = await self.bless_gatt_server.start()

        if not success:
            raise RuntimeError("GATT server couldn't be started")

        try:
            logger.info("GATT Server started")
            self.trigger.clear()
            await self.trigger.wait()
            logger.debug("GATT loop exiting")
            await self.bless_gatt_server.stop()

        except KeyboardInterrupt:
            logger.info("KeyboardInterrupt: Shutting Down")
        except Exception as e:
            logger.exception("GATT Server Crashed",
                             exc_info=e, stack_info=True)
            raise
        logger.info("GATT Server stopped")

    def _build_gatt():
        gatt: Dict = {
            ImprovUUID.SERVICE_UUID.value: {
                ImprovUUID.STATUS_UUID.value: {
                    "Properties": (GATTCharacteristicProperties.read |
                                   GATTCharacteristicProperties.notify),
                    "Permissions": (GATTAttributePermissions.readable |
                                    GATTAttributePermissions.writeable)
                },
                ImprovUUID.ERROR_UUID.value: {
                    "Properties": (GATTCharacteristicProperties.read |
                                   GATTCharacteristicProperties.notify),
                    "Permissions": (GATTAttributePermissions.readable |
                                    GATTAttributePermissions.writeable)
                },
                ImprovUUID.RPC_COMMAND_UUID.value: {
                    "Properties": (GATTCharacteristicProperties.read |
                                   GATTCharacteristicProperties.write |
                                   GATTCharacteristicProperties.write_without_response),
                    "Permissions": (GATTAttributePermissions.readable |
                                    GATTAttributePermissions.writeable)
                },
                ImprovUUID.RPC_RESULT_UUID.value: {
                    "Properties": (GATTCharacteristicProperties.read |
                                   GATTCharacteristicProperties.notify),
                    "Permissions": (GATTAttributePermissions.readable)
                },
                ImprovUUID.CAPABILITIES_UUID.value: {
                    "Properties": (GATTCharacteristicProperties.read),
                    "Permissions": (GATTAttributePermissions.readable)
                },
            }
        }
        return gatt

    def wifi_connect(ssid: str, passwd: str) -> Optional[list[str]]:
        ssid = ssid.decode('utf-8')
        passwd = passwd.decode('utf-8')
        logger.info(
            f"Connecting to '{ssid}' with password: '{passwd}'")
        if WifiManager.connectToWifi(ssid, passwd):
            networkConfig = WifiManager.getCurrentConfig()
            localServer = [
                f"http://{str(localIP.ip)}" for localIP in networkConfig.ips]
            logger.debug(
                f"Backend redirect IP/URL: {localServer}")
            return localServer
        return None

    def get_wifi_networks() -> Optional[list[str]]:
        ssids = []
        for s in WifiManager.scanForNetworks():
            ssids.append(s.ssid)
        return ssids

    def read_request(
            characteristic: BlessGATTCharacteristic,
            **kwargs
    ) -> bytearray:
        try:
            improv_char = ImprovUUID(characteristic.uuid)
            logger.info(f"Reading {improv_char} : {characteristic}")
        except Exception:
            logger.info(f"Reading {characteristic.uuid}")
            pass
        if characteristic.service_uuid == ImprovUUID.SERVICE_UUID.value:
            return GATTServer.getServer().improv_server.handle_read(characteristic.uuid)
        return characteristic.value

    def write_request(
        characteristic: BlessGATTCharacteristic,
        value: bytearray,
        **kwargs
    ):
        if characteristic.service_uuid == ImprovUUID.SERVICE_UUID.value:
            (target_uuid, target_values) = GATTServer.getServer().improv_server.handle_write(
                characteristic.uuid, value)
            if target_uuid != None and target_values != None:
                for value in target_values:
                    logger.debug(
                        f"Setting {ImprovUUID(target_uuid)} to {value}")
                    GATTServer.getServer().bless_gatt_server.get_characteristic(
                        target_uuid,
                    ).value = value
                    success = GATTServer.getServer().bless_gatt_server.update_value(
                        ImprovUUID.SERVICE_UUID.value,
                        target_uuid
                    )
                    if not success:
                        logger.warning(
                            f"Updating characteristic return status={success}")
