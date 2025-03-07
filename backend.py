from tornado.options import parse_command_line
import socketio
import tornado.log
import tornado.web
import tornado.ioloop
import time
import json
import os
import os.path
import pyprctl
import sentry_sdk

from ble_gatt import GATTServer
from wifi import WifiManager
from notifications import NotificationManager
from profiles import ProfileManager
from hostname import HostnameManager
from config import (
    MeticulousConfig,
    CONFIG_USER,
    CONFIG_SYSTEM,
    DEVICE_IDENTIFIER,
    MACHINE_SERIAL_NUMBER,
    UPDATE_CHANNEL,
)

from machine import Machine
from sounds import SoundPlayer
from imager import DiscImager
from ota import UpdateManager
from esp_serial.connection.emulation_data import EmulationData
from usb import USBManager

from api.api import API
from api.emulation import register_emulation_handlers
from api.web_ui import WEB_UI_HANDLER

from log import MeticulousLogger

from dbus_monitor import DBusMonitor

from api.machine import UpdateOSStatus

from timezone_manager import TimezoneManager

from ssh_manager import SSHManager
from telemetry_service import TelemetryService

logger = MeticulousLogger.getLogger(__name__)

tornado.log.access_log = MeticulousLogger.getLogger("tornado.access")
tornado.log.app_log = MeticulousLogger.getLogger("tornado.application")
tornado.log.gen_log = MeticulousLogger.getLogger("tornado.general")

PORT = int(os.getenv("PORT", "8080"))
DEBUG = os.getenv("DEBUG", "False").lower() in ("true", "1", "y")


sio = socketio.AsyncServer(cors_allowed_origins="*", async_mode="tornado")

UpdateOSStatus.setSio(sio)


@sio.event
def connect(sid, environ):
    logger.info("connect %s", sid)


@sio.event
def disconnect(sid):
    logger.info("disconnect %s", sid)


@sio.on("action")
def msg(sid, data):
    if data in Machine.ALLOWED_ESP_ACTIONS:
        Machine.action(action_event=data)
    else:
        logger.warning(f"Invalid action {data}")


@sio.on("notification")
def notification(sid, noti_json):
    notification = json.loads(noti_json)
    if "id" in notification and "response" in notification:
        NotificationManager.acknowledge_notification(
            notification["id"], notification["response"]
        )


@sio.on("profileHover")
async def forwardProfileHover(sid, data):
    logger.info(f"Hovering Profile {json.dumps(data, indent=1, sort_keys=False)}")
    await sio.emit("profileHover", data, skip_sid=sid)


@sio.on("calibrate")  # Use when calibration it is implemented
def calibrate(sid, data=True):
    know_weight = "100.0"
    current_weight = Machine.data_sensors.weight
    data = "calibration" + "," + know_weight + "," + str(current_weight)
    _input = "action," + data + "\x03"
    Machine.write(str.encode(_input))


async def live():
    SAMPLE_TIME = 0.1
    elapsed_time = 0
    i = 0
    _time = time.time()
    logger.info("Starting to emit machine data")

    # Store previous value of 'auto_preheat' to detect changes
    # previous_auto_preheat = MeticulousConfig[CONFIG_USER].get('auto_preheat', None)

    while True:

        elapsed_time = time.time() - _time
        if elapsed_time > 2 and not Machine.infoReady:
            _time = time.time()
            Machine.action("info")

        machine_status = {**Machine.data_sensors.to_sio()}
        # We can enrich the machines functionality from within the backend
        # as we know which profile was last loaded
        last_profile_entry = ProfileManager.get_last_profile()
        if last_profile_entry:
            profile = last_profile_entry["profile"]

            # In emulation mode the machine is unaware of its profile so we trick it here
            if (
                Machine.emulated
                and machine_status["profile"] == EmulationData.PROFILE_PLACEHOLDER
            ):
                if Machine.profileReady:
                    machine_status["profile"] = profile["name"]
                else:
                    machine_status["profile"] = "default"

            machine_status["loaded_profile"] = profile["name"]
            machine_status["id"] = profile["id"]
        else:
            machine_status["loaded_profile"] = None
            machine_status["id"] = None

        await sio.emit("status", machine_status)

        if Machine.sensor_sensors is not None:
            water_status_dict = (  # noqa: F841
                Machine.sensor_sensors.to_sio_water_status()
            )
            # water_status_value = water_status_dict["water_status"]
            # await sio.emit("water_status", water_status_value)

        if Machine.sensor_sensors is not None:
            await sio.emit("sensors", Machine.sensor_sensors.to_sio_temperatures())
            await sio.emit(
                "comunication", Machine.sensor_sensors.to_sio_communication()
            )
            await sio.emit("actuators", Machine.sensor_sensors.to_sio_actuators())
            await sio.emit(
                "accessories", Machine.sensor_sensors.to_sio_accessory_data()
            )

        # current_auto_preheat = MeticulousConfig[CONFIG_USER].get('auto_preheat')
        # if current_auto_preheat != previous_auto_preheat:
        #     Machine.write(str.encode("action,auto_preheat,"+str(current_auto_preheat)+"\x03"))
        #     previous_auto_preheat = current_auto_preheat

        await sio.sleep(SAMPLE_TIME)
        i = i + 1


def main():
    global dbus_object
    parse_command_line()

    pyprctl.set_name("Main")

    DBusMonitor.init()
    HostnameManager.init()
    UpdateManager.setChannel(MeticulousConfig[CONFIG_USER][UPDATE_CHANNEL])

    try:
        # Context is arbitrary data that will be sent with every event
        sentry_sdk.set_context("build-info", UpdateManager.getRepositoryInfo())

        # Tags are indexed and searchable
        sentry_sdk.set_tag("build-timestamp", UpdateManager.getBuildTimestamp())
        sentry_sdk.set_tag("build-channel", UpdateManager.getImageChannel())
        sentry_sdk.set_tag(
            "machine", "".join(MeticulousConfig[CONFIG_SYSTEM][DEVICE_IDENTIFIER])
        )
        sentry_sdk.set_tag(
            "serial", MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER]
        )
    except Exception as e:
        logger.error(f"Failed to set sentry context: {e}")

    Machine.init(sio)
    SSHManager.init()

    USBManager.init()
    GATTServer.getServer().start()

    WifiManager.init()
    NotificationManager.init(sio)
    ProfileManager.init(sio)
    SoundPlayer.init(emulation=Machine.emulated)

    # Check for mapped timezones json
    TimezoneManager.init()
    TelemetryService.init()

    MeticulousConfig.setSIO(sio)

    handlers = [
        (r"/socket.io/", socketio.get_tornado_handler(sio)),
    ]

    if Machine.emulated and not WifiManager.networking_available():
        register_emulation_handlers()

    handlers.extend(API.get_routes())

    handlers.extend(WEB_UI_HANDLER)

    app = tornado.web.Application(
        handlers,
        debug=DEBUG,
    )

    app.listen(PORT)

    sio.start_background_task(live)

    DiscImager.flash_if_required()
    tornado.ioloop.IOLoop.current().start()
