import os
import threading
import asyncio
import time

from config import *

from esp_serial.data import *
from esp_serial.connection.usb_serial_connection import USBSerialConnection
from esp_serial.connection.fika_serial_connection import FikaSerialConnection
from esp_serial.connection.emulator_serial_connection import EmulatorSerialConnection

from shot_manager import ShotManager

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)

# can be from [FIKA, USB, EMULATOR / EMULATION]
BACKEND=os.getenv("BACKEND", 'FIKA').upper()

class Machine:
    _connection = None
    _thread = None
    _stopESPcomm = False
    _sio = None

    infoReady = False

    data_sensors = ShotData()
    sensor_sensors = None
    esp_info = None
    reset_count = 0
    shot_start_time = 0
    emulated = False

    def init(sio):
        Machine._sio = sio
        match(BACKEND):
            case "USB":
                Machine._connection = USBSerialConnection('/dev/ttyUSB0')
            case "EMULATOR" | "EMULATION":
                Machine._connection = EmulatorSerialConnection()
                Machine.emulated = True
            # Everything else is proper fika Connection
            case "FIKA" | _ :
                Machine._connection = FikaSerialConnection('/dev/ttymxc0')

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
        
        logger.info("Starting to listen for esp32 messages")

        while True:
            if Machine._stopESPcomm:
                time.sleep(0.1)
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
                    Machine.reset_count+=1

                if Machine.reset_count >= 3:
                    logger.warning("The ESP seems to be resetting, sending update now")
                    Machine.startUpdate()
                    Machine.reset_count = 0

                match(data_str_sensors):
                    # FIXME: This should be replace in the firmware with an "Event," prefix for cleanliness
                    case ["CCW" | "CW" | "push" | "pu_d" | "elng" | "ta_d" | "ta_l" | "strt"] as ev:
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
                    is_idle = data.status == MachineStatus.IDLE
                    is_infusion = data.status == MachineStatus.INFUSION
                    is_preinfusion = data.status == MachineStatus.PREINFUSION
                    is_spring = data.status == MachineStatus.SPRING
                    is_purge = data.status == MachineStatus.PURGE
                    was_preparing = old_status == MachineStatus.CLOSING_VALVE

                    if (was_preparing and (is_preinfusion or is_infusion or is_spring)):
                        time_flag = True
                        shot_start_time = time.time()
                        logger.info("shot start_time: {:.1f}".format(shot_start_time))
                        ShotManager.start()

                    if (is_idle or is_purge):
                        time_flag = False
                        ShotManager.stop()

                    if (time_flag):
                        time_passed = int((time.time() - shot_start_time) * 1000.0)
                        Machine.data_sensors = data.clone_with_time(time_passed)
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
                        ShotManager.handleSensorData(Machine.sensor_sensors, time_passed)

                if info is not None:
                    Machine.esp_info = info
                    Machine.reset_count = 0
                    Machine.infoReady = True

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
        if not Machine._stopESPcomm:
            _input = "action,"+action_event+"\x03"
            Machine._connection.port.write(str.encode(_input))

    def write(content):
        if not Machine._stopESPcomm:
            Machine._connection.port.write(content)

    def reset():
        Machine._connection.reset()