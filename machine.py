import asyncio
import hashlib
import json
import os
import random
import threading
import time

from config import *
from esp_serial.connection.emulator_serial_connection import EmulatorSerialConnection
from esp_serial.connection.fika_serial_connection import FikaSerialConnection
from esp_serial.connection.usb_serial_connection import USBSerialConnection
from esp_serial.data import (
    ButtonEventData,
    ButtonEventEnum,
    ESPInfo,
    MachineStatus,
    SensorData,
    ShotData,
)
from esp_serial.esp_tool_wrapper import ESPToolWrapper
from log import MeticulousLogger
from notifications import Notification, NotificationManager, NotificationResponse
from packaging import version
from shot_debug_manager import ShotDebugManager
from shot_manager import ShotManager
from sounds import SoundPlayer, Sounds

logger = MeticulousLogger.getLogger(__name__)

# can be from [FIKA, USB, EMULATOR / EMULATION]
BACKEND = os.getenv("BACKEND", "FIKA").upper()


class Machine:
    _connection = None
    _thread = None
    _stopESPcomm = False
    _sio = None

    infoReady = False
    profileReady = False

    data_sensors: ShotData = ShotData()
    sensor_sensors: SensorData = None
    esp_info = None
    reset_count = 0
    shot_start_time = 0
    emulated = False
    firmware_available = None
    firmware_running = None
    startTime = None

    is_idle = True

    def init(sio):
        Machine._sio = sio
        Machine.firmware_available = Machine._parseVersionString(
            ESPToolWrapper.get_version_from_firmware()
        )

        if Machine._connection is not None:
            logger.warning("Machine.init was called twice!")
            return

        # FIXME!
        if not MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER]:
            MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER] = int(
                random.random() * 999999
            )
            colors = ["black", "white", "pink"]
            MeticulousConfig[CONFIG_SYSTEM][MACHINE_COLOR] = random.choice(colors)
            MeticulousConfig.save()

        match (BACKEND):
            case "USB":
                Machine._connection = USBSerialConnection("/dev/ttyUSB0")
            case "EMULATOR" | "EMULATION":
                Machine._connection = EmulatorSerialConnection()
                Machine.emulated = True
            # Everything else is proper fika Connection
            case "FIKA" | _:
                Machine._connection = FikaSerialConnection("/dev/ttymxc0")

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
                r = self.buf[: i + 1]
                self.buf = self.buf[i + 1 :]
                return r
            while not Machine._stopESPcomm:
                i = max(1, min(2048, self.s.in_waiting))
                data = self.s.read(i)
                i = data.find(b"\n")
                if i >= 0:
                    r = self.buf + data[: i + 1]
                    self.buf[0:] = data[i + 1 :]
                    return r
                else:
                    self.buf.extend(data)
            return self.buf

    async def _read_data():
        Machine.shot_start_time = time.time()
        Machine._connection.port.reset_input_buffer()
        Machine._connection.port.write(b"32\n")
        uart = Machine.ReadLine(Machine._connection.port)

        old_status = MachineStatus.IDLE
        time_flag = False
        info_requested = False
        time_passed = 0

        logger.info("Starting to listen for esp32 messages")
        Machine.startTime = time.time()
        while True:
            if Machine._stopESPcomm:
                await asyncio.sleep(0.1)
                Machine.startTime = time.time()
                continue

            data = uart.readline()
            if len(data) > 0:
                # data_bit = bytes(data)
                try:
                    data_str = data.decode("utf-8")
                except:
                    logger.info(f"decoding fails, message: {data}")
                    continue

                if (
                    old_status != MachineStatus.IDLE and data_str.startswith("Sensor")
                ) or MeticulousConfig[CONFIG_LOGGING][LOGGING_SENSOR_MESSAGES]:
                    logger.info(data_str.strip("\r\n"))

                data_str_sensors = data_str.strip("\r\n").split(",")

                # potential message types
                button_event = None
                sensor = None
                data = None
                info = None

                if (
                    data_str.startswith("rst:0x")
                    and "boot:0x16 (SPI_FAST_FLASH_BOOT)" in data_str
                ):
                    Machine.reset_count += 1
                    Machine.startTime = time.time()
                    Machine.esp_info = None
                    info_requested = False
                    Machine.infoReady = False
                    Machine.profileReady = False

                if Machine.reset_count >= 3:
                    logger.warning("The ESP seems to be resetting, sending update now")
                    Machine.startUpdate()
                    Machine.reset_count = 0

                if (
                    Machine.infoReady
                    and not info_requested
                    and Machine.esp_info is None
                ):
                    logger.info(
                        "Machine has not provided us with a firmware version yet. Requesting now"
                    )
                    Machine.action("info")
                    info_requested = True

                if time.time() - Machine.startTime > 60 and not Machine.infoReady:
                    if MeticulousConfig[CONFIG_USER][DISALLOW_FIRMWARE_FLASHING]:
                        logger.warning(
                            "The ESP never send an info, but user requested no updates!"
                        )
                    else:
                        logger.warning(
                            "The ESP never send an info, flashing latest firmware to be sure"
                        )
                        Machine.startUpdate()

                match (data_str_sensors):
                    # FIXME: This should be replace in the firmware with an "Event," prefix for cleanliness
                    case [
                        "CCW"
                        | "CW"
                        | "push"
                        | "pu_d"
                        | "elng"
                        | "ta_d"
                        | "ta_l"
                        | "strt"
                    ] as ev:
                        button_event = ButtonEventData.from_args(ev)
                    case ["Event", *eventData]:
                        button_event = ButtonEventData.from_args(eventData)
                    case ["Data", *dataArgs]:
                        data = ShotData.from_args(dataArgs)
                    case ["Sensors", colorCodedString]:
                        sensor = SensorData.from_color_coded_args(colorCodedString)
                    case ["Sensors", *sensorArgs]:
                        sensor = SensorData.from_args(sensorArgs)
                    case ["ESPInfo", *infoArgs]:
                        info = ESPInfo.from_args(infoArgs)
                    case [*_]:
                        logger.info(data_str.strip("\r\n"))

                if data is not None:
                    Machine.is_idle = data.status == MachineStatus.IDLE
                    is_purge = data.status == MachineStatus.PURGE
                    is_retracting = data.status == MachineStatus.RETRACTING
                    is_preparing = data.status == MachineStatus.CLOSING_VALVE
                    is_heating = data.status == MachineStatus.HEATING

                    if is_preparing and data.status != old_status:
                        time_flag = True
                        shot_start_time = time.time()
                        logger.info("shot start_time: {:.1f}".format(shot_start_time))
                        ShotManager.start()
                        SoundPlayer.play_event_sound(Sounds.BREWING_START)

                    if old_status == MachineStatus.IDLE and not Machine.is_idle:
                        ShotDebugManager.start()
                        if is_heating or is_preparing or is_retracting:
                            time_passed = 0

                    if Machine.is_idle and old_status != MachineStatus.IDLE:
                        SoundPlayer.play_event_sound(Sounds.IDLE)

                    if Machine.is_idle or is_purge or is_retracting:
                        if time_flag == True:
                            SoundPlayer.play_event_sound(Sounds.BREWING_END)
                        time_flag = False
                        ShotManager.stop()

                    if Machine.is_idle:
                        ShotDebugManager.stop()

                    if is_heating and old_status != MachineStatus.HEATING:
                        time_passed = 0
                        SoundPlayer.play_event_sound(Sounds.HEATING_START)

                    if old_status == MachineStatus.HEATING and not is_heating:
                        SoundPlayer.play_event_sound(Sounds.HEATING_END)

                    if time_flag:
                        time_passed = int((time.time() - shot_start_time) * 1000.0)
                        Machine.data_sensors = data.clone_with_time_and_state(
                            time_passed, True
                        )
                        ShotManager.handleShotData(Machine.data_sensors)
                    else:
                        Machine.data_sensors = data.clone_with_time_and_state(
                            time_passed, False
                        )

                    ShotDebugManager.handleShotData(Machine.data_sensors)
                    old_status = Machine.data_sensors.status
                    Machine.infoReady = True

                if sensor is not None:
                    Machine.sensor_sensors = sensor
                    Machine.reset_count = 0
                    ShotDebugManager.handleSensorData(Machine.sensor_sensors)
                    if time_flag:
                        ShotManager.handleSensorData(Machine.sensor_sensors)

                if info is not None:
                    Machine.esp_info = info
                    Machine.reset_count = 0
                    Machine.infoReady = True
                    info_requested = False
                    Machine.firmware_running = Machine._parseVersionString(
                        info.firmwareV
                    )
                    logger.info(
                        f"ESPInfo running firmware version:   {Machine.firmware_running} on pinout version {Machine.esp_info.espPinout}"
                    )
                    logger.info(
                        f"Backend available firmware version: {Machine.firmware_available}"
                    )
                    needs_update = False
                    if (
                        Machine.firmware_available is not None
                        and Machine.firmware_available is not None
                    ):
                        if (
                            Machine.firmware_running["Release"]
                            < Machine.firmware_available["Release"]
                        ):
                            needs_update = True
                        if (
                            Machine.firmware_running["Release"]
                            == Machine.firmware_available["Release"]
                        ):
                            if (
                                Machine.firmware_running["ExtraCommits"]
                                < Machine.firmware_available["ExtraCommits"]
                            ):
                                needs_update = True

                    if (
                        needs_update
                        and not MeticulousConfig[CONFIG_USER][
                            DISALLOW_FIRMWARE_FLASHING
                        ]
                    ):
                        info_string = f"Firmware {Machine.firmware_running.get('Release')}-{Machine.firmware_running['ExtraCommits']} is outdated, upgrading"
                        logger.info(info_string)

                        Machine.startUpdate()

                if button_event is not None:
                    if (
                        button_event.event is not ButtonEventEnum.ENCODER_CLOCKWISE
                        and button_event.event
                        is not ButtonEventEnum.ENCODER_COUNTERCLOCKWISE
                    ):
                        logger.debug(f"Button Event recieved: {button_event}")

                    await Machine._sio.emit("button", button_event.to_sio())

                # FIXME this should be a callback to the frontends in the future
                if (
                    button_event is not None
                    and button_event.event is ButtonEventEnum.ENCODER_DOUBLE
                ):
                    logger.info("DOUBLE ENCODER, Returning to idle")
                    Machine.return_to_idle()

    def startUpdate():
        updateNotification = Notification(
            "Upgrading system realtime core. This will take around 20 seconds. The machines buttons will be disabled during the upgrade",
            [""],
        )
        NotificationManager.add_notification(updateNotification)

        Machine._stopESPcomm = True
        error_msg = Machine._connection.sendUpdate()
        Machine._stopESPcomm = False

        if error_msg:
            updateNotification.message = f"Realtime core upgrade failed: {error_msg}. The machine will ensure a good state on next start. If you encounter any errors please reach out to product support!"
        else:
            updateNotification.message = f"The realtime core was upgraded sucessfully! Buttons will be enabled again in around 5 seconds"
        updateNotification.respone_options = [NotificationResponse.OK]
        NotificationManager.add_notification(updateNotification)
        return error_msg

    def return_to_idle():
        if Machine.data_sensors.status != "idle":
            Machine.action("stop")
        SoundPlayer.play_event_sound(Sounds.ABORT)

    def action(action_event) -> bool:
        logger.info(f"sending action,{action_event}")
        if action_event == "start" and not Machine.profileReady:
            logger.warning("No profile loaded, sending last loaded profile to esp32")
            from profile import ProfileManager

            last_profile = ProfileManager.get_last_profile()
            if last_profile is None:
                logger.error("No known last profile which could be sent to the esp32")
                return False

            ProfileManager.send_profile_to_esp32(last_profile["profile"])

        machine_msg = f"action,{action_event}\x03"
        Machine.writeStr(machine_msg)
        return True

    def writeStr(content):
        Machine.write(str.encode(content))

    def write(content):
        if not Machine._stopESPcomm:
            Machine._connection.port.write(content)

    def reset():
        Machine._connection.reset()
        Machine.infoReady = False
        Machine.profileReady = False
        Machine.startTime = time.time()

    def send_json_with_hash(json_obj):
        json_string = json.dumps(json_obj)
        json_data = "json\n" + json_string + "\x03"

        logger.debug("JSON to stream to the machine:")
        logger.debug(json_data)

        json_hash = hashlib.md5(json_data[5:-1].encode("utf-8")).hexdigest()

        logger.info(f"JSON Hash: {json_hash}")

        start = time.time()
        Machine.write("hash,".encode("utf-8"))
        Machine.write(json_hash.encode("utf-8"))
        Machine.write("\x03".encode("utf-8"))
        Machine.write(json_data.encode("utf-8"))
        end = time.time()
        time_ms = (end - start) * 1000
        if time_ms > 10:
            time_str = f"{int(time_ms)} ms"
        else:
            time_str = f"{int(time_ms*1000)} ns"
        logger.info(f"Streaming profile to ESP32 took {time_str}")
        Machine.profileReady = True

    def _parseVersionString(version_str: str):
        release = None
        ncommits = 0
        sha = ""
        modifier = ""
        if version_str is None or version_str == "":
            return None

        components = version_str.strip().split("-")
        try:
            release = version.Version(components.pop(0))
            if len(components) > 0:
                ncommits = components.pop(0)
            if len(components) > 0:
                sha = components.pop(0)
            if len(components) > 0:
                modifier = components.pop(0)
            return {
                "Release": release,
                "ExtraCommits": ncommits,
                "SHA": sha,
                "Local": modifier,
            }
        except Exception as e:
            logger.warning(
                "Failed parse firmware version:", exc_info=e, stack_info=True
            )
            return None
