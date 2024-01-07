from modes.italian_1_0.italian_1_0 import generate_italian_1_0
from modes.dashboard_1_0.dashboard_1_0 import generate_dashboard_1_0
from tornado.options import define, options, parse_command_line
import socketio
import tornado.web
import tornado.ioloop
import threading
import time
import json
import os
import os.path
import hashlib
import version as backend
import subprocess
import asyncio

from esp_serial.connection.usb_serial_connection import USBSerialConnection
from esp_serial.connection.fika_serial_connection import FikaSerialConnection
from esp_serial.connection.emulator_serial_connection import EmulatorSerialConnection

from esp_serial.data import *

from ble_gatt import GATTServer
from wifi import WifiManager
from notifications import Notification, NotificationManager, NotificationResponse
from profile import ProfileManager
from config import *

from api.profiles import PROFILE_HANDLER
from api.notifications import NOTIFICATIONS_HANDLER
from api.wifi import WIFI_HANDLER
from api.emulation import EMULATED_WIFI_HANDLER
from api.update import UPDATE_HANDLER

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)
esp32_logger = MeticulousLogger.getLogger("esp32")

user_path=os.path.expanduser("~/")

sendInfoToFront = False
infoReady = False
lastJSON_source = "LCD"

connection = None
ble_gatt_server: GATTServer = None

class ReadLine:
    def __init__(self, s):
        self.buf = bytearray()
        self.s = s
    
    def readline(self):
        global stopESPcomm
        i = self.buf.find(b"\n")
        if i >= 0:
            r = self.buf[:i+1]
            self.buf = self.buf[i+1:]
            return r
        while not stopESPcomm:
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

#thread variables
data_thread = None
send_data_thread = None
stopESPcomm = False

def gatherVersionInfo():
    global infoSolicited
    software_info["name"] = "Meticulous Espresso"
    software_info["backendV"] = backend.VERSION

    # #OBTENEMOS SU VERSION USANDO LOS COMANDOS DPKG y GREP
    command = f'dpkg --list | grep meticulous-ui'
    result = subprocess.run(command, shell=True, capture_output=True, text=True)
    try:
        lcd_version = result.stdout.split()[2]
    except IndexError:
        logger.warning("LCD DialApp is not installed")
        lcd_version = "0.0.0"
    infoSolicited = True

    software_info["lcdV"] = lcd_version

define("port", default=8080, help="run on the given port", type=int)
define("debug", default=False, help="run in debug mode")

sio = socketio.AsyncServer(cors_allowed_origins='*', async_mode='tornado')

data_sensors = ShotData()
sensor_sensors = None
esp_info = None

software_info = {
    "name": "Meticulous Espresso",
    "lcdV": 3,
}


def return_to_idle():
    if (data_sensors.status != "idle"):
        _input = "action,"+"stop"+"\x03"
        if(connection.port != None): connection.port.write(str.encode(_input))
    logger.info("DOUBLE ENCODER, Returning to idle")


def send_json_hash(json_obj):
    json_string = json.dumps(json_obj)
    json_data = "json\n" + json_string + "\x03"
    #proof = detect_source(json_string,json_data)
    #logger.info(proof)
    logger.debug(json_data)
    # logger.info(json_data)
    json_hash = hashlib.md5(json_data[5:-1].encode('utf-8')).hexdigest()
    logger.debug("hash_enviado: " + json_hash + "\n")
    logger.info(f"hash: {json_hash}")
    if(connection.port != None): connection.port.write("hash ".encode("utf-8"))
    if(connection.port != None): connection.port.write(json_hash.encode("utf-8"))
    if(connection.port != None): connection.port.write("\x03".encode("utf-8"))
    if(connection.port != None): connection.port.write(json_data.encode("utf-8"))

def detect_source(json_data):
    
    source = ""
    try:
        source = json_data["source"]
        source = source.upper()
    except:
        source = "LCD"
    return source

@sio.event
def connect(sid, environ):
    logger.info('connect %s', sid)

@sio.event
def disconnect(sid):
    logger.info('disconnect %s', sid)

@sio.on('action')
def msg(sid, data):
    if data == "start":
        time.sleep(0.5)
        data = "action,"+data+"\x03"
        logger.info(data)
        if(connection.port != None): connection.port.write(data.encode("utf-8"))
    else:
        time.sleep(0.05)
        data = "action,"+data+"\x03"
        logger.info(data)
        if(connection.port != None): connection.port.write(data.encode("utf-8"))

@sio.on('askForInfo')
def setSendInfo(sid):
    global sendInfoToFront
    sendInfoToFront = True

@sio.on('stopInfo')
def StopInfo(sid):
    global sendInfoToFront
    sendInfoToFront = False

@sio.on('parameters')
def msg(sid, data):
    global lastJSON_source
    send_json_hash(data)
    lastJSON_source = detect_source(data)
    logger.info(lastJSON_source)

@sio.on('send_profile')
async def forwardJSON(sid,data):
    logger.info(json.dumps(data, indent=1, sort_keys=False))
    await sio.emit('save_profile', data)

@sio.on('calibrate') #Use when calibration it is implemented
def msg(sid, data=True):
    know_weight = "100.0"
    current_weight = data_sensors.weight
    data ="calibration"+","+know_weight+","+str(current_weight)
    _input = "action,"+data+"\x03"
    if(connection.port != None): connection.port.write(str.encode(_input))

@sio.on('feed_profile')
async def feed_profile(sid, data):
    logger.info("Received JSON:", data)  # Print the received JSON data
    # Deserialize the JSON
    obj = json.loads(data)
    # Extract and print the value of "kind"
    kind_value = obj.get('kind', None)
    if kind_value:
        if kind_value =="italian_1_0":
            logger.info("Is Italian 1.0")
            json_result = generate_italian_1_0(obj) #<class 'str'>
            logger.info(json_result)
            obj_json = json.loads(json_result) #<class 'dict'>
            send_json_hash(obj_json)
            time.sleep(5)
            _input = "action,"+"start"+"\x03"
            connection.port.write(str.encode(_input))
            
        if kind_value =="dashboard_1_0":
            logger.info("Is Dashboard 1.0")
            action_value = obj.get('action', None)
            if action_value == "to_play":
                json_result = generate_dashboard_1_0(obj)  #<class 'str'>
                logger.info(json_result)
                obj_json = json.loads(json_result) #<class 'dict'>
                send_json_hash(obj_json)
                time.sleep(5)
                _input = "action,"+"start"+"\x03"
                connection.port.write(str.encode(_input))
                logger.info("Se envio start")
            elif action_value == "save_in_dial":
                # The following remove the property "action" from the json_data
                obj.pop("action")
                # emit the event to save the profile using "save_in_dial" event
                logger.info("Se envio save_in_dial: ",obj)
                await sio.emit("save_in_dial",json.dumps(obj))
        
        if kind_value =="spring_1_0":
            logger.info("Spring 1.0")
    else:
        print("The 'kind' key is not present in the received JSON.")

async def read_arduino():
    global infoReady
    global stopESPcomm
    global data_sensors
    global sensor_sensors
    global esp_info

    reset_count = 0
    shot_start_time = time.time()

    connection.port.reset_input_buffer()
    connection.port.write(b'32\n')
    uart = ReadLine(connection.port)

    old_status = MachineStatus.IDLE
    time_flag = False

    while True:
        if stopESPcomm:
            time.sleep(0.1)
            continue

        data = uart.readline()
        if len(data) > 0:
            # data_bit = bytes(data)
            try:
                data_str = data.decode('utf-8')
            except:
                esp32_logger.info(f"decoding fails, message: {data}")
                continue

            if (old_status != MachineStatus.IDLE and data_str.startswith("Sensor")) or MeticulousConfig[CONFIG_LOGGING][LOGGING_SENSOR_MESSAGES]:
                esp32_logger.info(data_str.strip("\r\n"))

            data_str_sensors = data_str.strip("\r\n").split(',')

            # potential message types
            button_event = None
            sensor = None
            data = None
            info = None

            if data_str.startswith("rst:0x") and "boot:0x16 (SPI_FAST_FLASH_BOOT)" in data_str:
                reset_count+=1

            if reset_count >= 3:
                logger.warning("The ESP seems to be resetting, sending update now")
                startUpdate()
                reset_count = 0

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
                    esp32_logger.info(data_str.strip("\r\n"))

            if data is not None:
                is_idle = data.status == MachineStatus.IDLE
                is_infusion = data.status == MachineStatus.INFUSION
                is_preinfusion = data.status == MachineStatus.PREINFUSION
                is_spring = data.status == MachineStatus.SPRING
                was_preparing = old_status == MachineStatus.CLOSING_VALVE

                if (was_preparing and (is_preinfusion or is_infusion or is_spring)):
                    time_flag = True
                    shot_start_time = time.time()
                    logger.info("shot start_time: {:.1f}".format(shot_start_time))

                if (is_idle):
                    time_flag = False

                if (time_flag):
                    data_sensors = data.clone_with_time(time.time() - shot_start_time)
                else:
                    data_sensors = data
                old_status = data_sensors.status
                infoReady = True

            if sensor is not None:
                sensor_sensors = sensor#
                reset_count = 0
                infoReady = True

            if info is not None:
                esp_info = info
                reset_count = 0
                infoReady = True

            if button_event is not None:
                await sio.emit("button", button_event.to_sio())

            # FIXME this should be a callback to the frontends in the future
            if button_event is not None and button_event.event is ButtonEventEnum.ENCODER_DOUBLE:
                return_to_idle()
    
def data_treatment():
    global connection
    if(connection.port != None):
        loop = asyncio.new_event_loop()
        asyncio.set_event_loop(loop)

        loop.run_until_complete(read_arduino())
        loop.close()

async def live():

    global sendInfoToFront
    global infoReady
    global infoSolicited
    global lastJSON_source

    process_started = False
    SAMPLE_TIME = 0.1
    elapsed_time = 0
    i = 0
    _time = time.time()
    while True:

        elapsed_time = time.time() - _time
        if infoSolicited and (elapsed_time > 2 and not infoReady):
            _time = time.time()
            _solicitud = "action,info\x03"
            if(connection.port != None and not  stopESPcomm): connection.port.write(str.encode(_solicitud))

        await sio.emit("status", {**data_sensors.to_sio(), "source": lastJSON_source,})

        if sendInfoToFront:
            logger.info("Sending info to front")
            if sensor_sensors is not None:
                await sio.emit("sensors", sensor_sensors.to_sio_temperatures())
                await sio.emit("comunication", sensor_sensors.to_sio_communication())
                await sio.emit("actuators", sensor_sensors.to_sio_actuators())
            if esp_info is not None:
                # FIXME change to lowercase info for consistency
                await sio.emit("info", {**software_info, **esp_info.to_sio()})
        await sio.sleep(SAMPLE_TIME)
        i = i + 1

def send_data():
    global lastJSON_source
    global ble_gatt_server

    while (True):
        print("> ", end="")
        try:
            _input = input()
        except EOFError:
            logger.warning("no STDIN attached, not listening to commands!")
            break

        if _input == "reset":
            connection.reset()

        elif _input == "show":
            MeticulousConfig[CONFIG_LOGGING][LOGGING_SENSOR_MESSAGES] = True
            MeticulousConfig.save()
        
        elif _input == "hide":
            MeticulousConfig[CONFIG_LOGGING][LOGGING_SENSOR_MESSAGES] = False
            MeticulousConfig.save()

        elif _input== "json":
            with open('fika.json','r') as openfile:
                json_file = json.load(openfile)
                # json_data = json.dumps(json_file, indent=1, sort_keys=False)
                # logger.info(json_data)
                send_json_hash(json_file)
                lastJSON_source = detect_source(json_file)
                json_data=""
                json_file=""
            

        elif _input=="tare" or _input=="stop" or _input=="purge" or _input=="home" or _input=="start" :
            _input = "action,"+_input+"\x03"
            if(connection.port != None): connection.port.write(str.encode(_input))
            
        elif _input == "test":
            previous_sensor_status = MeticulousConfig[CONFIG_LOGGING][LOGGING_SENSOR_MESSAGES]
            MeticulousConfig[CONFIG_LOGGING][LOGGING_SENSOR_MESSAGES] = True
            for i in range(0,10):
                _input = "action,"+"purge"+"\x03"
                if(connection.port != None): connection.port.write(str.encode(_input))
                time.sleep(15)
                logger.info(_input)
                _input = "action,"+"home"+"\x03"
                if(connection.port != None): connection.port.write(str.encode(_input))
                time.sleep(15)
                contador = "Numero de prueba: "+str(i+1)
                logger.info(_input)
                logger.info(contador)
            MeticulousConfig[CONFIG_LOGGING][LOGGING_SENSOR_MESSAGES] = previous_sensor_status

        elif _input[:11] == "calibration":
             _input = "action,"+_input+"\x03"
             if(connection.port != None): connection.port.write(str.encode(_input))

        elif _input.startswith("update"):
            startUpdate()

        elif _input.startswith("wifi"):
            if ble_gatt_server.is_running():
                ble_gatt_server.stop()
            else:
                ble_gatt_server.start()

        elif _input.startswith("notification"):
            notification = _input[12:]
            NotificationManager.add_notification(Notification(notification, [NotificationResponse.OK]))


        elif _input != "":
            logger.info(f"Unknown command: \"{_input}\"")
            pass

# can be from [FIKA, USB, EMULATOR / EMULATION]
BACKEND=os.getenv("BACKEND", 'FIKA').upper()

def main():
    global data_thread
    global send_data_thread
    global connection
    global ble_gatt_server

    emulation = False

    parse_command_line()

    gatherVersionInfo()

    match(BACKEND):
        case "USB":
            connection = USBSerialConnection('/dev/ttyUSB0')
        case "EMULATOR" | "EMULATION":
            connection = EmulatorSerialConnection()
            emulation = True
        # Everything else is proper fika connection
        case "FIKA" | _ :
            connection = FikaSerialConnection('/dev/ttymxc0')

    ble_gatt_server = GATTServer.getServer()

    data_thread = threading.Thread(target=data_treatment)
    data_thread.start()

    send_data_thread = threading.Thread(target=send_data) 
    send_data_thread.start()

    WifiManager.init()
    NotificationManager.init(sio)

    handlers = [
            (r"/socket.io/", socketio.get_tornado_handler(sio)),
        ]

    handlers.extend(PROFILE_HANDLER)
    handlers.extend(NOTIFICATIONS_HANDLER)
    handlers.extend(UPDATE_HANDLER)

    if emulation:
        handlers.extend(EMULATED_WIFI_HANDLER)
    else:
        handlers.extend(WIFI_HANDLER)

    app = tornado.web.Application(
        handlers,
        debug=options.debug,
    )

    app.listen(options.port)
    
    sio.start_background_task(live)
    tornado.ioloop.IOLoop.current().start()

def menu():    
    logger.info("Hi, please select the option you want: ")
    logger.info("reset --> reset the esp32")
    logger.info("[tare, stop, start, purge, home] --> Send the corresponding command on the esp32")
    logger.info("json --> Send the latest fika.json from local storage to the ESP32")
    logger.info("show --> Show data received from the esp32")
    logger.info("hide --> Stop showing data received from esp32 except for status messages")
    logger.info("test --> Moves the engine 10 times from purge to home and displays the value of the sensors")
    logger.info("calibration,<known_weight>,<measured_weight> --> Calibrate the weight")

def startUpdate():
    global stopESPcomm

    stopESPcomm = True
    connection.sendUpdate()
    stopESPcomm = False

if __name__ == "__main__":
    try:
        menu()
        main()
    except Exception as e:
        logger.exception("main() failed", exc_info=e, stack_info=True)
        exit(1)
