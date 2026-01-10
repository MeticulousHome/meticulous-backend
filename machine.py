import asyncio
import hashlib
import json
import os
from named_thread import NamedThread
import time
from enum import Enum
import sentry_sdk
import random
import string
from ota import UpdateManager
from packaging import version
import subprocess
from monitoring.motor_power_monitoring import motor_energy_calculator

from config import (
    CONFIG_LOGGING,
    CONFIG_SYSTEM,
    CONFIG_USER,
    SSH_ENABLED,
    CONFIG_MANUFACTURING,
    DISALLOW_FIRMWARE_FLASHING,
    LOGGING_SENSOR_MESSAGES,
    MACHINE_COLOR,
    MACHINE_SERIAL_NUMBER,
    MACHINE_BUILD_DATE,
    MACHINE_BATCH_NUMBER,
    MACHINE_HEAT_ON_BOOT,
    MeticulousConfig,
)
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
    MachineNotify,
    HeaterTimeoutInfo,
)
from esp_serial.esp_tool_wrapper import ESPToolWrapper
from log import MeticulousLogger
from notifications import Notification, NotificationManager, NotificationResponse
from shot_debug_manager import ShotDebugManager
from shot_manager import ShotManager
from sounds import SoundPlayer, Sounds

from manufacturing import FORCE_MANUFACTURING_ENABLED_KEY, LAST_BOOT_MODE_KEY


def toggle_sentry(enabled):
    # Disable sentry if we are on manufacturing mode
    sentry_client = sentry_sdk.get_client()
    if sentry_client:
        if enabled:
            logger.info("Sentry disabled: In Manufacturing mode")
        else:
            logger.info("Sentry enabled: Manufacturing mode disabled")
        sentry_client.options["enabled"] = enabled
    else:
        logger.error(
            f'Cannot get sentry client to toggle to {"enabled" if enabled else "disabled"}'
        )


logger = MeticulousLogger.getLogger(__name__)

# can be from [FIKA, USB, EMULATOR / EMULATION]
BACKEND = os.getenv("BACKEND", "FIKA").upper()


class esp_nvs_keys(Enum):
    color = "color_key"
    serial_number = "serial_number_key"
    batch_number = "batch_number_key"
    build_date = "build_date_key"


class Machine:

    ALLOWED_BACKEND_ACTIONS = ["reset", "abort"]
    ALLOWED_ESP_ACTIONS = [
        "start",
        "stop",
        "tare",
        "scale_master_calibration",
        "preheat",
        "continue",
        "home",
        "purge",
        "continue",
    ]

    _connection = None
    _thread = None
    _stopESPcomm = False
    _sio = None
    _espNotification = Notification("", [NotificationResponse.OK])

    heater_timeout_info: HeaterTimeoutInfo = None

    infoReady = False
    profileReady = False
    oldProfileReady = False

    data_sensors: ShotData = ShotData(
        state=MachineStatus.IDLE, status=MachineStatus.IDLE, profile=MachineStatus.IDLE
    )
    sensor_sensors: SensorData = None
    esp_info = None
    reset_count = 0
    shot_start_time = 0
    emulated = False
    firmware_available = None
    firmware_running = None
    startTime = None

    is_idle = True

    enable_manufacturing = False

    is_first_normal_boot = False

    stable_start_timestamp = None

    stable_time_threshold = 2.0

    @staticmethod
    def get_somrev():
        # Get the raw output from i2cget
        result = subprocess.run(
            ["i2cget", "-f", "-y", "0x0", "0x52", "0x1e"],
            stdout=subprocess.PIPE,
            text=True,
            check=True,
        )
        raw_output = result.stdout.strip()
        decimal_output = int(raw_output, 16)
        major = ((decimal_output & 0xE0) >> 5) + 1
        minor = decimal_output & 0x1F
        return f"{major}.{minor}"

    def on_first_normal_boot():
        """
        Function to execute things only after exiting tha manufactuing mode
        """
        from ssh_manager import SSHManager

        SSHManager.set_ssh_state(False)
        MeticulousConfig[CONFIG_USER][SSH_ENABLED] = False
        MeticulousConfig.save()

    def toggle_manufacturing_mode(enabled):
        Machine.enable_manufacturing = enabled
        toggle_sentry(enabled=not enabled)
        MeticulousConfig[CONFIG_MANUFACTURING][LAST_BOOT_MODE_KEY] = (
            "manufacturing" if enabled else "normal"
        )
        MeticulousConfig.save()

    def validate_manufacturing():
        if MeticulousConfig[CONFIG_MANUFACTURING][FORCE_MANUFACTURING_ENABLED_KEY]:
            Machine.toggle_manufacturing_mode(enabled=True)
            return

        # Look for the S/N in the .yml file, if there is a SN we dont enable it by default
        serial: str | None = MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER]

        # This should never happend but some older machines had their serial as a number and dont convert properly
        if isinstance(serial, int):
            serial = str(serial)
            MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER] = serial
            MeticulousConfig.save()

        if (
            serial is not None
            and serial != ""
            and serial != "NOT_ASSIGNED"
            and not serial.startswith("999")
        ):
            # if we are not in manufacturing mode, check if we were in the previous boot
            Machine.is_first_normal_boot = (
                MeticulousConfig[CONFIG_MANUFACTURING][LAST_BOOT_MODE_KEY]
                == "manufacturing"
            )
            return

        Machine.toggle_manufacturing_mode(enabled=True)

    @staticmethod
    def generate_random_serial():
        random_digits = "".join(random.choices(string.digits, k=5))
        return f"999{random_digits}"

    def check_machine_alive():
        if not Machine.infoReady:
            if MeticulousConfig[CONFIG_USER][DISALLOW_FIRMWARE_FLASHING]:
                logger.warning(
                    "The ESP never send an info, but user requested no updates!"
                )
            else:
                logger.warning(
                    "The ESP never send an info, flashing latest firmware to be sure"
                )
                Machine.startUpdate()
        else:
            logger.info("The ESP is alive")

    def init(sio):
        Machine._sio = sio
        Machine.firmware_available = Machine._parseVersionString(
            ESPToolWrapper.get_version_from_firmware()
        )

        # If we dont have a serial we still want to be able to show ... something
        serial = MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER]
        if serial is None or serial == "":
            serial = Machine.generate_random_serial()
            MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER] = serial
            MeticulousConfig.save()
        Machine.validate_manufacturing()

        if Machine._connection is not None:
            logger.warning("Machine.init was called twice!")
            return

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

        if not Machine.emulated:
            som = Machine.get_somrev()
            logger.info(f"SOM revision: {som}")

        def startLoop():
            loop = asyncio.new_event_loop()
            asyncio.set_event_loop(loop)

            loop.run_until_complete(Machine._read_data())
            loop.close()

        def flashingEsp():
            time.sleep(60)
            Machine.check_machine_alive()

        Machine._thread = NamedThread("MachineSerial", target=startLoop)
        Machine._flashingThread = NamedThread("FlashingEsp", target=flashingEsp)
        Machine._thread.start()
        Machine._flashingThread.start()

        # if the we are on the first non-manufacturing boot
        if Machine.is_first_normal_boot:
            Machine.on_first_normal_boot()

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

    async def _read_data():  # noqa: C901
        Machine.shot_start_time = time.time()
        Machine._connection.port.reset_input_buffer()
        Machine._connection.port.write(b"32\n")
        uart = Machine.ReadLine(Machine._connection.port)

        old_status = MachineStatus.IDLE
        old_ready = False
        time_flag = False
        info_requested = False
        time_passed = 0
        profile_time = 0
        emulated_firmware = False
        previous_preheat_remaining = None

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
                except Exception:
                    logger.info(f"decoding fails, message: {data}")
                    continue

                if MeticulousConfig[CONFIG_LOGGING][LOGGING_SENSOR_MESSAGES]:
                    logger.info(data_str.strip("\r\n"))

                data_str_sensors = data_str.strip("\r\n").split(",")

                # potential message types
                button_event = None
                sensor = None
                data = None
                info = None
                notify = None

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

                match (data_str_sensors):
                    # FIXME: This should be replace in the firmware with an "Event," prefix
                    # for cleanliness
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
                    case ["Notify", *notifyArgs]:
                        notify = MachineNotify(
                            notifyArgs[0], ",".join(notifyArgs[1:]).replace(";", "\n")
                        )

                    case ["HeaterTimeoutInfo", *timeoutArgs]:
                        try:
                            heater_timeout_info = HeaterTimeoutInfo.from_args(
                                timeoutArgs
                            )
                            Machine.heater_timeout_info = heater_timeout_info
                            await Machine._sio.emit(
                                "heater_status", heater_timeout_info.preheat_remaining
                            )
                            if (
                                heater_timeout_info.preheat_remaining == 0
                                and previous_preheat_remaining != 0
                            ):
                                logger.info("Heater_status: off")
                            previous_preheat_remaining = (
                                heater_timeout_info.preheat_remaining
                            )

                        except Exception as e:
                            logger.error(
                                f"Error processing HeaterTimeoutInfo: {e}",
                                exc_info=True,
                            )
                    case [*_]:
                        logger.info(data_str.strip("\r\n"))

                old_ready = Machine.infoReady

                if data is not None:
                    Machine.is_idle = data.status == MachineStatus.IDLE
                    is_purge = data.status == MachineStatus.PURGE
                    is_retracting = data.status == MachineStatus.RETRACTING
                    is_preparing = data.status == MachineStatus.CLOSING_VALVE
                    was_preparing = old_status == MachineStatus.CLOSING_VALVE
                    is_heating = data.status == MachineStatus.HEATING
                    is_starting = data.status == MachineStatus.STARTING

                    # A shot started
                    if was_preparing and data.status != old_status:
                        time_flag = True
                        shot_start_time = time.time()
                        logger.info("shot start_time: {:.1f}".format(shot_start_time))
                        ShotManager.start()
                        SoundPlayer.play_event_sound(Sounds.BREWING_START)
                    elif time_flag:
                        # A shot could have ended
                        if Machine.is_idle or is_purge:
                            time_flag = False

                        # After retracting the shot is always over. No matter what, during retracting we wait for a stable weight
                        if old_status == MachineStatus.RETRACTING and not is_retracting:
                            time_flag = False
                            ShotManager.stop()

                        if is_retracting:
                            if Machine.stable_start_timestamp is not None:
                                time_flag = (
                                    time.time() - Machine.stable_start_timestamp
                                ) < Machine.stable_time_threshold
                                if not data.stable_weight:
                                    Machine.stable_start_timestamp = None
                            else:
                                if data.stable_weight:
                                    Machine.stable_start_timestamp = time.time()

                        if time_flag is False:
                            now_time = time.time()
                            if Machine.stable_start_timestamp is not None:
                                logger.info(
                                    f"shot ended at {now_time} with a stable weight time of: {now_time - Machine.stable_start_timestamp}s"
                                )
                                Machine.stable_start_timestamp = None
                            else:
                                logger.info("shot ended with weight unstable")
                            SoundPlayer.play_event_sound(Sounds.BREWING_END)
                            ShotManager.stop()

                    if Machine.is_idle and old_status != MachineStatus.IDLE:
                        Machine.profileReady = False

                    if old_status == MachineStatus.IDLE and not Machine.is_idle:
                        if is_heating or is_preparing or is_retracting or is_starting:
                            time_passed = 0
                            profile_time = 0

                    if Machine.profileReady and not Machine.oldProfileReady:
                        ShotDebugManager.start()
                    if not Machine.profileReady and Machine.oldProfileReady:
                        ShotDebugManager.stop()

                    Machine.oldProfileReady = Machine.profileReady

                    if Machine.is_idle and old_status != MachineStatus.IDLE:
                        SoundPlayer.play_event_sound(Sounds.IDLE)

                    if is_heating and old_status != MachineStatus.HEATING:
                        time_passed = 0
                        profile_time = 0
                        SoundPlayer.play_event_sound(Sounds.HEATING_START)

                    if old_status == MachineStatus.HEATING and not is_heating:
                        SoundPlayer.play_event_sound(Sounds.HEATING_END)

                    if time_flag:
                        time_passed = int((time.time() - shot_start_time) * 1000.0)
                        profile_time = time_passed
                        if is_retracting:
                            profile_time = ShotManager.handleExtractionEnd(time_passed)
                        Machine.data_sensors = data.clone_with_time_and_state(
                            time_passed, True, profile_time
                        )
                        ShotManager.handleShotData(Machine.data_sensors)

                    else:
                        Machine.data_sensors = data.clone_with_time_and_state(
                            time_passed, False, profile_time
                        )

                    ShotDebugManager.handleShotData(Machine.data_sensors)
                    old_status = Machine.data_sensors.status
                    Machine.infoReady = True

                if sensor is not None:

                    def stopMotorIfHot(_sensorData: SensorData):
                        MAX_ENERGY_ALLOWED = 12000
                        energy_consumed_by_motor = (
                            motor_energy_calculator.calculate_motor_energy(
                                _sensorData, Machine.data_sensors
                            )
                        )
                        if energy_consumed_by_motor >= MAX_ENERGY_ALLOWED:
                            logger.warning(
                                f"Motor might be hot, has consumed {energy_consumed_by_motor}. Stopping profile"
                            )
                            motorHotNotification = Notification(
                                f"Motor might be hot, has consumed {energy_consumed_by_motor}. Stopping profile"
                            )
                            motorHotNotification.respone_options = [
                                NotificationResponse.OK
                            ]
                            NotificationManager.add_notification(motorHotNotification)
                            Machine.end_profile()

                    Machine.sensor_sensors = sensor
                    Machine.reset_count = 0
                    ShotDebugManager.handleSensorData(Machine.sensor_sensors)
                    if time_flag:
                        ShotManager.handleSensorData(Machine.sensor_sensors)
                    stopMotorIfHot(Machine.sensor_sensors)

                if info is not None:
                    Machine.esp_info = info
                    Machine.reset_count = 0
                    Machine.infoReady = True
                    info_requested = False
                    Machine.firmware_running = Machine._parseVersionString(
                        info.firmwareV
                    )

                    if (
                        info.serialNumber != ""
                        and info.serialNumber != "NOT_ASSIGNED"
                        and info.color != ""
                        and info.color != "NOT_ASSIGNED"
                        and info.batchNumber != ""
                        and info.batchNumber != "NOT_ASSIGNED"
                        and info.buildDate != ""
                        and info.buildDate != "NOT_ASSIGNED"
                    ):
                        MeticulousConfig[CONFIG_SYSTEM][
                            MACHINE_SERIAL_NUMBER
                        ] = info.serialNumber
                        MeticulousConfig[CONFIG_SYSTEM][MACHINE_COLOR] = info.color
                        MeticulousConfig[CONFIG_SYSTEM][
                            MACHINE_BATCH_NUMBER
                        ] = info.batchNumber
                        MeticulousConfig[CONFIG_SYSTEM][
                            MACHINE_BUILD_DATE
                        ] = info.buildDate

                        MeticulousConfig.save()

                    # Enable / Disable manufacturing mode based on ESP answer
                    serial_assigned = (
                        MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER]
                        is not None
                    )
                    if Machine.enable_manufacturing != serial_assigned:
                        if not MeticulousConfig[CONFIG_MANUFACTURING][
                            FORCE_MANUFACTURING_ENABLED_KEY
                        ]:
                            Machine.toggle_manufacturing_mode(enabled=False)

                    if not emulated_firmware:
                        logger.info(
                            f"ESPInfo running firmware version:   {Machine.firmware_running} on pinout version {Machine.esp_info.espPinout} Machine_color: {Machine.esp_info.color} Serial_number: {Machine.esp_info.serialNumber} Batch_number: {Machine.esp_info.batchNumber} Build_date: {Machine.esp_info.buildDate}"
                        )
                        logger.info(
                            f"Backend available firmware version: {Machine.firmware_available}"
                        )
                        emulated_firmware = Machine.emulated
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
                            != Machine.firmware_available["Release"]
                        ) and UpdateManager.is_changed:
                            needs_update = True

                        if (
                            Machine.firmware_running["Release"]
                            == Machine.firmware_available["Release"]
                        ):
                            try:
                                running_extra = int(
                                    Machine.firmware_running["ExtraCommits"]
                                )
                                available_extra = int(
                                    Machine.firmware_available["ExtraCommits"]
                                )
                                if running_extra < available_extra:
                                    needs_update = True
                                if (
                                    running_extra != available_extra
                                ) and UpdateManager.is_changed:
                                    needs_update = True
                            except Exception:
                                pass

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
                    Machine.end_profile()

                if (
                    not old_ready
                    and Machine.infoReady
                    and MeticulousConfig[CONFIG_USER][MACHINE_HEAT_ON_BOOT]
                ):
                    if Machine.data_sensors.status == MachineStatus.IDLE:
                        logger.info("Tell the machine to preheat")
                        logger.warning("NOT IMPLEMENTED YET")
                        # Machine.action("preheat")

                if notify is not None:
                    if notify.notificationType == "acaia_msg":
                        responseOptions = []
                    else:
                        responseOptions = [NotificationResponse.OK]
                    if Machine._espNotification.acknowledged:
                        Machine._espNotification = Notification(
                            notify.message, responseOptions
                        )
                    else:
                        Machine._espNotification.message = notify.message
                        Machine._espNotification.respone_options = responseOptions
                    logger.info(
                        f"New Notification from ESP: {Machine._espNotification.message}"
                    )
                    NotificationManager.add_notification(Machine._espNotification)

    def startScaleMasterCalibration():
        Machine.action("scale_master_calibration")

    def startUpdate():

        Machine._stopESPcomm = True
        error_msg = Machine._connection.sendUpdate()
        Machine._stopESPcomm = False

        if error_msg:
            updateNotification = Notification(
                f"Realtime core upgrade failed: {error_msg}. The machine will ensure a good state on next start. If you encounter any errors please reach out to product support!"
            )
            updateNotification.respone_options = [NotificationResponse.OK]
            NotificationManager.add_notification(updateNotification)
        return error_msg

    def end_profile():
        if Machine.data_sensors.status == "idle":
            return
        logger.info("Ending profile due to user request")
        if (
            Machine.data_sensors.state == "brewing"
            and Machine.data_sensors.status
            not in [
                "heating",
                "Pour water and click to continue",
                "click to start",
                "purge",
            ]
        ):
            Machine.action("home")
        else:
            Machine.action("stop")
        SoundPlayer.play_event_sound(Sounds.ABORT)

    def action(action_event) -> bool:
        logger.info(f"sending action,{action_event}")
        if action_event == "start" and not Machine.profileReady:
            logger.warning("No profile loaded, sending last loaded profile to esp32")
            from profiles import ProfileManager

            last_profile = ProfileManager.get_last_profile()
            if last_profile is None:
                logger.error("No known last profile which could be sent to the esp32")
                return False

            ProfileManager.send_profile_to_esp32(last_profile["profile"])

        if action_event == "home" or action_event == "purge":
            Machine.profileReady = True

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
        while True:
            if ShotDebugManager._current_data is not None:
                break
        with ShotDebugManager.clear_current_data_lock:
            ShotDebugManager._current_data.nodeJSON = json_obj

    def setSerial(color, serial, batch_number, build_date):
        write_request = "nvs_request,write,"
        Machine.write(
            (write_request + esp_nvs_keys.color.value + "," + color + "\x03").encode(
                "utf-8"
            )
        )
        Machine.write(
            (
                write_request + esp_nvs_keys.serial_number.value + "," + serial + "\x03"
            ).encode("utf-8")
        )
        Machine.write(
            (
                write_request
                + esp_nvs_keys.batch_number.value
                + ","
                + batch_number
                + "\x03"
            ).encode("utf-8")
        )
        Machine.write(
            (
                write_request
                + esp_nvs_keys.build_date.value
                + ","
                + build_date
                + "\x03"
            ).encode("utf-8")
        )

        serialNotification = Notification(
            f"""
Serial number: {serial}\n
Batch number: {batch_number}\n
Color: {color}\n
Build Date: {build_date}
            """,
            responses=[NotificationResponse.OK],
        )
        NotificationManager.add_notification(serialNotification)
        MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER] = serial
        MeticulousConfig[CONFIG_SYSTEM][MACHINE_COLOR] = color
        MeticulousConfig[CONFIG_SYSTEM][MACHINE_BATCH_NUMBER] = batch_number
        MeticulousConfig[CONFIG_SYSTEM][MACHINE_BUILD_DATE] = build_date

        MeticulousConfig.save()
        # TODO FIXME IMPLEMENT THIS!!!!

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
