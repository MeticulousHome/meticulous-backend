import os
import threading
import asyncio
import time
from packaging import version

from config import *

from esp_serial.data import *
from esp_serial.connection.usb_serial_connection import USBSerialConnection
from esp_serial.connection.fika_serial_connection import FikaSerialConnection
from esp_serial.connection.emulator_serial_connection import EmulatorSerialConnection
from esp_serial.esp_tool_wrapper import ESPToolWrapper

from shot_manager import ShotManager

from notifications import NotificationManager, Notification, NotificationResponse

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)

# can be from [FIKA, USB, EMULATOR / EMULATION]
BACKEND = os.getenv("BACKEND", 'FIKA').upper()


class Machine:
    _connection = None
    _thread = None
    _stopESPcomm = False
    _sio = None
    _updateNotification = None

    infoReady = False

    data_sensors = ShotData()
    sensor_sensors = None
    esp_info = None
    reset_count = 0
    shot_start_time = 0
    emulated = False
    firmware_available = None
    firmware_running = None
    startTime = None

    def init(sio):
        Machine._sio = sio
        Machine.firmware_available = Machine._parseVersionString(
            ESPToolWrapper.get_version_from_firmware())

        if Machine._connection is not None:
            logger.warning("Machine.init was called twice!")
            return

        match(BACKEND):
            case "USB":
                Machine._connection = USBSerialConnection('/dev/ttyUSB0')
            case "EMULATOR" | "EMULATION":
                Machine._connection = EmulatorSerialConnection()
                Machine.emulated = True
            # Everything else is proper fika Connection
            case "FIKA" | _:
                Machine._connection = FikaSerialConnection('/dev/ttymxc0')

        Machine.writeStr("\x03")
        Machine.action("info")

        def startLoop():
            loop = asyncio.new_event_loop()
            asyncio.set_event_loop(loop)

            loop.run_until_complete(Machine._read_data())
            loop.close()

        Machine._thread = threading.Thread(target=startLoop)
        Machine._thread.start()

    class ReadLine:
        def __init__(self, s):
            self.buf = bytearray()
            self.s = s

        def readline(self):
            i = self.buf.find(b"\n")
            if i >= 0:
                r = self.buf[:i+1]
                self.buf = self.buf[i+1:]
                return r
            while not Machine._stopESPcomm:
                i = max(1, min(2048, self.s.in_waiting))
                data = self.s.read(i)
                i = data.find(b"\n")
                if i >= 0:
                    r = self.buf + data[:i+1]
                    self.buf[0:] = data[i+1:]
                    return r
                else:
                    self.buf.extend(data)
            return self.buf

    async def _read_data():
        Machine.shot_start_time = time.time()
        Machine._connection.port.reset_input_buffer()
        Machine._connection.port.write(b'32\n')
        uart = Machine.ReadLine(Machine._connection.port)

        old_status = MachineStatus.IDLE
        time_flag = False
        info_requested = False

        logger.info("Starting to listen for esp32 messages")
        Machine.startTime = time.time()
        while True:
            if Machine._stopESPcomm:
                time.sleep(0.1)
                Machine.startTime = time.time()
                continue

            data = uart.readline()
            if len(data) > 0:
                # data_bit = bytes(data)
                try:
                    data_str = data.decode('utf-8')
                except:
                    logger.info(f"decoding fails, message: {data}")
                    continue

                if (old_status != MachineStatus.IDLE and data_str.startswith("Sensor")) or MeticulousConfig[CONFIG_LOGGING][LOGGING_SENSOR_MESSAGES]:
                    logger.info(data_str.strip("\r\n"))

                data_str_sensors = data_str.strip("\r\n").split(',')

                # potential message types
                button_event = None
                sensor = None
                data = None
                info = None

                if data_str.startswith("rst:0x") and "boot:0x16 (SPI_FAST_FLASH_BOOT)" in data_str:
                    Machine.reset_count += 1
                    Machine.startTime = time.time()
                    Machine.esp_info = None
                    info_requested = False
                    Machine.infoReady = False

                if Machine.reset_count >= 3:
                    logger.warning(
                        "The ESP seems to be resetting, sending update now")
                    Machine.startUpdate()
                    Machine.reset_count = 0

                if Machine.infoReady and not info_requested and Machine.esp_info is None:
                    logger.info("Machine has not provided us with a firmware version yet. Requesting now")
                    Machine.action("info")
                    info_requested = True

                if time.time() - Machine.startTime > 60 and not Machine.infoReady:
                    if MeticulousConfig[CONFIG_USER][DISALLOW_FIRMWARE_FLASHING]:
                        logger.warning(
                            "The ESP never send an info, but user requested no updates!")
                    else:
                        logger.warning(
                            "The ESP never send an info, flashing latest firmware to be sure")
                        Machine.startUpdate()

                match(data_str_sensors):
                    # FIXME: This should be replace in the firmware with an "Event," prefix for cleanliness
                    case ["CCW" | "CW" | "push" | "pu_d" | "elng" | "ta_d" | "ta_l" | "strt"] as ev:
                        button_event = ButtonEventData.from_args(ev)
                    case ["Event", *eventData]:
                        button_event = ButtonEventData.from_args(eventData)
                    case ["Data", *dataArgs]:
                        data = ShotData.from_args(dataArgs)
                    case ["Sensors", colorCodedString]:
                        sensor = SensorData.from_color_coded_args(
                            colorCodedString)
                    case ["Sensors", *sensorArgs]:
                        sensor = SensorData.from_args(sensorArgs)
                    case ["ESPInfo", *infoArgs]:
                        info = ESPInfo.from_args(infoArgs)
                    case [*_]:
                        logger.info(data_str.strip("\r\n"))

                if data is not None:
                    is_idle = data.status == MachineStatus.IDLE
                    is_infusion = data.status == MachineStatus.INFUSION
                    is_preinfusion = data.status == MachineStatus.PREINFUSION
                    is_spring = data.status == MachineStatus.SPRING
                    is_purge = data.status == MachineStatus.PURGE
                    was_preparing = old_status == MachineStatus.CLOSING_VALVE

                    if (was_preparing and (is_preinfusion or is_infusion or is_spring)):
                        time_flag = True
                        shot_start_time = time.time()
                        logger.info(
                            "shot start_time: {:.1f}".format(shot_start_time))
                        ShotManager.start()

                    if (is_idle or is_purge):
                        time_flag = False
                        ShotManager.stop()

                    if (time_flag):
                        time_passed = int(
                            (time.time() - shot_start_time) * 1000.0)
                        Machine.data_sensors = data.clone_with_time(
                            time_passed)
                        ShotManager.handleShotData(Machine.data_sensors)
                    else:
                        Machine.data_sensors = data
                    old_status = Machine.data_sensors.status
                    Machine.infoReady = True

                if sensor is not None:
                    Machine.sensors = sensor
                    Machine.reset_count = 0
                    Machine.actioninfoReady = True
                    if (time_flag):
                        ShotManager.handleSensorData(
                            Machine.sensor_sensors, time_passed)

                if info is not None:
                    Machine.esp_info = info
                    Machine.reset_count = 0
                    Machine.infoReady = True
                    info_requested = False
                    Machine.firmware_running = Machine._parseVersionString(
                        info.firmwareV
                    )
                    logger.info(
                        f"ESPInfo running firmware version:   {Machine.firmware_running}")
                    logger.info(
                        f"Backend available firmware version: {Machine.firmware_available}")
                    needs_update = False
                    if Machine.firmware_available is not None and Machine.firmware_available is not None:
                        if Machine.firmware_running["Release"] < Machine.firmware_available["Release"]:
                            needs_update = True
                        if Machine.firmware_running["Release"] == Machine.firmware_available["Release"]:
                            if Machine.firmware_running["ExtraCommits"] < Machine.firmware_available["ExtraCommits"]:
                                needs_update = True

                    if needs_update and not MeticulousConfig[CONFIG_USER][DISALLOW_FIRMWARE_FLASHING]:
                        info_string = f"Firmware {Machine.firmware_running.get('Release')}-{Machine.firmware_running['ExtraCommits']} is outdated, upgrading"
                        logger.info(info_string)
                        Machine._updateNotification = Notification(
                            info_string, [NotificationResponse.OK])
                        await NotificationManager.add_notification(
                            Machine._updateNotification)
                        Machine.startUpdate()

                if button_event is not None:
                    await Machine._sio.emit("button", button_event.to_sio())

                # FIXME this should be a callback to the frontends in the future
                if button_event is not None and button_event.event is ButtonEventEnum.ENCODER_DOUBLE:
                    logger.info("DOUBLE ENCODER, Returning to idle")
                    Machine.return_to_idle()

    def startUpdate():
        Machine._stopESPcomm = True
        Machine._connection.sendUpdate()
        Machine._stopESPcomm = False

    def return_to_idle():
        if (Machine.data_sensors.status != "idle"):
            Machine.action("stop")

    def action(action_event):
        logger.info(f"sending action,{action_event}")
        machine_msg = f"action,{action_event}\x03"
        Machine.writeStr(machine_msg)

    def writeStr(content):
        Machine.write(str.encode(content))

    def write(content):
        if not Machine._stopESPcomm:
            Machine._connection.port.write(content)

    def reset():
        Machine._connection.reset()
        Machine.infoReady = False
        Machine.startTime = time.time()

    def _parseVersionString(version_str: str):
        release = None
        ncommits = 0
        sha = ""
        modifier = ""
        if version_str is None or version_str == "":
            return None

        components = version_str.strip().split('-')
        try:
            release = version.Version(components.pop(0))
            if len(components) > 0:
                ncommits = components.pop(0)
            if len(components) > 0:
                sha = components.pop(0)
            if len(components) > 0:
                modifier = components.pop(0)
            return {"Release": release, "ExtraCommits": ncommits, "SHA": sha, "Local": modifier}
        except Exception as e:
            logger.warning("Failed parse firmware version:",
                           exc_info=e, stack_info=True)
            return None
