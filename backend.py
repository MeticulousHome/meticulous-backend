from tornado.options import parse_command_line
import socketio
import tornado.log
import tornado.web
import tornado.ioloop
from named_thread import NamedThread
import time
import json
import os
import os.path
import version as backend
import subprocess
import pyprctl
import asyncio

from esp_serial.data import ButtonEventData

from ble_gatt import GATTServer
from wifi import WifiManager
from notifications import Notification, NotificationManager
from profiles import ProfileManager
from hostname import HostnameManager
from config import (
    MeticulousConfig,
    CONFIG_USER,
    UPDATE_CHANNEL,
    CONFIG_LOGGING,
    LOGGING_SENSOR_MESSAGES,
)
from machine import Machine
from sounds import SoundPlayer
from imager import DiscImager
from ota import UpdateManager
from shot_manager import ShotManager
from esp_serial.connection.emulation_data import EmulationData

from api.api import API
from api.emulation import register_emulation_handlers
from api.web_ui import WEB_UI_HANDLER

from log import MeticulousLogger

from dbus_monitor import DBusMonitor

logger = MeticulousLogger.getLogger(__name__)

tornado.log.access_log = MeticulousLogger.getLogger("tornado.access")
tornado.log.app_log = MeticulousLogger.getLogger("tornado.application")
tornado.log.gen_log = MeticulousLogger.getLogger("tornado.general")

PORT = int(os.getenv("PORT", "8080"))
DEBUG = os.getenv("DEBUG", "False").lower() in ("true", "1", "y")


def gatherVersionInfo():
    global infoSolicited
    software_info["name"] = "Meticulous Espresso"
    software_info["backendV"] = backend.VERSION

    # #OBTENEMOS SU VERSION USANDO LOS COMANDOS DPKG y GREP
    command = "dpkg --list | grep meticulous-ui"
    result = subprocess.run(command, shell=True, capture_output=True, text=True)
    try:
        lcd_version = result.stdout.split()[2]
    except IndexError:
        logger.warning("LCD DialApp is not installed")
        lcd_version = "0.0.0"
    infoSolicited = True

    software_info["lcdV"] = lcd_version


sio = socketio.AsyncServer(cors_allowed_origins="*", async_mode="tornado")

software_info = {
    "name": "Meticulous Espresso",
    "lcdV": 3,
}


@sio.event
def connect(sid, environ):
    logger.info("connect %s", sid)


@sio.event
def disconnect(sid):
    logger.info("disconnect %s", sid)


@sio.on("set_heater_timeout")
def set_timeout(sid, timeout):
    logger.info(f"Setting timeout to {timeout}")
    data = f"heater_timeout,{timeout}\x03"
    logger.info(data)
    Machine.write(data.encode("utf-8"))


@sio.on("action")
def msg(sid, data):
    if data == "start":
        time.sleep(0.5)
        data = "action," + data + "\x03"
        logger.info(data)
        Machine.write(data.encode("utf-8"))
    else:
        time.sleep(0.05)
        data = "action," + data + "\x03"
        logger.info(data)
        Machine.write(data.encode("utf-8"))


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


send_data_thread = None


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
        if Machine.esp_info is not None:
            await sio.emit("info", {**software_info, **Machine.esp_info.to_sio()})

        # current_auto_preheat = MeticulousConfig[CONFIG_USER].get('auto_preheat')
        # if current_auto_preheat != previous_auto_preheat:
        #     Machine.write(str.encode("action,auto_preheat,"+str(current_auto_preheat)+"\x03"))
        #     previous_auto_preheat = current_auto_preheat

        await sio.sleep(SAMPLE_TIME)
        i = i + 1


def send_data_loop():
    loop = asyncio.new_event_loop()
    asyncio.set_event_loop(loop)

    loop.run_until_complete(send_data())
    loop.close()


async def send_data():  # noqa: C901
    noti = Notification("", ["Ok", "Not okay"])
    while True:
        print("> ", end="")
        try:
            _input = input()
        except EOFError:
            logger.warning("no STDIN attached, not listening to commands!")
            break

        if _input == "reset":
            Machine.reset()

        elif _input == "show":
            MeticulousConfig[CONFIG_LOGGING][LOGGING_SENSOR_MESSAGES] = True
            MeticulousConfig.save()

        elif _input == "hide":
            MeticulousConfig[CONFIG_LOGGING][LOGGING_SENSOR_MESSAGES] = False
            MeticulousConfig.save()
        elif (
            _input == "tare"
            or _input == "stop"
            or _input == "purge"
            or _input == "home"
            or _input == "start"
        ):
            Machine.action(_input)

        elif _input == "test":
            previous_sensor_status = MeticulousConfig[CONFIG_LOGGING][
                LOGGING_SENSOR_MESSAGES
            ]
            MeticulousConfig[CONFIG_LOGGING][LOGGING_SENSOR_MESSAGES] = True
            for i in range(0, 10):
                _input = "action," + "purge" + "\x03"
                Machine.write(str.encode(_input))
                await asyncio.sleep(15)
                logger.info(_input)
                _input = "action," + "home" + "\x03"
                Machine.write(str.encode(_input))
                await asyncio.sleep(15)
                contador = "Numero de prueba: " + str(i + 1)
                logger.info(_input)
                logger.info(contador)
            MeticulousConfig[CONFIG_LOGGING][
                LOGGING_SENSOR_MESSAGES
            ] = previous_sensor_status

        elif _input[:11] == "calibration":
            _input = "action," + _input + "\x03"
            Machine.write(str.encode(_input))

        elif _input.startswith("update"):
            Machine.startUpdate()

        elif _input.startswith("notification"):
            notification = _input[12:]
            # noti.message = notification
            # noti.add_qrcode("Hello asjkdljlasjjkdsajkldasljkasdljk")
            noti = Notification(
                notification,
            )
            NotificationManager.add_notification(noti)
        elif _input == "l" or _input == "CCW":
            await sio.emit("button", ButtonEventData.from_args(["CCW"]).to_sio())
        elif _input == "r" or _input == "CW":
            await sio.emit("button", ButtonEventData.from_args(["CW"]).to_sio())
        elif _input == "e" or _input == "push":
            await sio.emit("button", ButtonEventData.from_args(["push"]).to_sio())
        elif _input == "d" or _input == "pu_d":
            await sio.emit("button", ButtonEventData.from_args(["pu_d"]).to_sio())
        elif _input == "t" or _input == "ta_d":
            await sio.emit("button", ButtonEventData.from_args(["ta_d"]).to_sio())
        elif _input == "s" or _input == "ta_l":
            await sio.emit("button", ButtonEventData.from_args(["ta_l"]).to_sio())
        elif _input == "ta_sl":
            await sio.emit("button", ButtonEventData.from_args(["ta_sl"]).to_sio())
        elif _input == "pr":
            await sio.emit(
                "button", ButtonEventData.from_args(["encoder_button_pressed"]).to_sio()
            )
        elif _input == "re":
            await sio.emit(
                "button",
                ButtonEventData.from_args(["encoder_button_released"]).to_sio(),
            )


def main():
    global send_data_thread
    global dbus_object
    parse_command_line()

    pyprctl.set_name("Main")

    gatherVersionInfo()
    DBusMonitor.init()
    HostnameManager.init()
    UpdateManager.setChannel(MeticulousConfig[CONFIG_USER][UPDATE_CHANNEL])

    Machine.init(sio)

    send_data_thread = NamedThread("SendSocketIO", target=send_data_loop)
    send_data_thread.start()

    GATTServer.getServer().start()

    WifiManager.init()
    NotificationManager.init(sio)
    ProfileManager.init(sio)
    ShotManager.init()
    SoundPlayer.init(emulation=Machine.emulated)
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
