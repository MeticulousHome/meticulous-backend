from modes.italian_1_0.italian_1_0 import generate_italian_1_0
from modes.dashboard_1_0.dashboard_1_0 import generate_dashboard_1_0
from tornado.options import define, options, parse_command_line
import socketio
import tornado.web
import tornado.ioloop
import traceback
import threading
import time
import json
# import RPi.GPIO as GPIO          ################# Debera haber un if que confirme en el entorno (raspberry o VAR-SOM-MX8M-NANO)
import gpiod                       ################# En base a ello instalara la libreria correspondiente (RPi.GPIO o gpiod)
#from dotenv import load_dotenv #################!!!!!!!!!!!NO hay librerias en som
from datetime import datetime
import os
import os.path
from operator import itemgetter
import hashlib
import version as backend
import subprocess
import base64
import asyncio

from esp_serial.connection.usb_serial_connection import USBSerialConnection
from esp_serial.connection.fika_serial_connection import FikaSerialConnection
from esp_serial.data import *

from ble_gatt import GATTServer

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

user_path=os.path.expanduser("~/")

usaFormatoDeColores = True

sendInfoToFront = False

infoReady = False

lastJSON_source = "LCD"

reboot_flag = False

connection = None
ble_gatt_server: GATTServer = None

IPC_path = f'{user_path}/ipc'                              # directory for the InterProcess Communication pipes
pipe1 = None
pipe2 = None
pipe2_path = f'{IPC_path}/pipe2'
pipe1_path = f'{IPC_path}/pipe1'
IPC_message = bytes()
#VERSION INFORMATION

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

ping_thread = None
watcher_listen_thread = None

def gatherVersionInfo():
    global infoSolicited
    software_info["name"] = "Meticulous Espresso"
    software_info["backendV"] = backend.VERSION

    # #OBTENEMOS EL NOMBRE DE LA APLICACION DE LA LCD
    # auxFile = open(os.path.expanduser("~/.xsession"))
    # lcd_ui_name = auxFile.read().split('\n')[2].split()[1]

    # #OBTENEMOS SU VERSION USANDO LOS COMANDOS DPKG y GREP
    command = f'dpkg --list | grep meticulous-ui'
    result = subprocess.run(command, shell=True, capture_output=True, text=True)
    lcd_version = result.stdout.split()[2]
    # lcd_version = 1.0 #HARDCODED!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!
    ##############################################Provisionalmente y al no haber una version de la LCD, se asigna la version 1.0
    infoSolicited = True

    software_info["lcdV"] = lcd_version
    software_info["dashboardV"] = 1.0
    software_info["firmwareV"] = 0.0

    #SOLICITAMOS LA VERSION DE FIRMWARE A LA ESP

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
    
#sendInfoToFront

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

@sio.on('toggle-fans')
def toggleFans(sid, data):
    _solicitud = ""
    if data:
        logger.info("fans on")
        _solicitud ="action,fans-on\x03"
    else:
        logger.info("fans off")
        _solicitud = "action,fans-off\x03"
    if(connection.port != None): connection.port.write(str.encode(_solicitud))
    software_info["fanStatus"] = 'on' if data else 'off'

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

@sio.on('preset')
def msg(sid, data):
    global lastJSON_source
    # data = data + "hola mundo"
    if (data == "breville"):
        preset="breville.json"
    
    elif (data == "cube"):
        preset="cube.json"

    elif (data == "diletta"):
        preset="diletta.json"

    elif (data == "flair"):
        preset="flair.json"

    elif (data == "la-pavoni"):
        preset="la-pavoni.json"

    elif (data == "rocket"):
        preset="rocket.json"

    else:
        logger.info("Preset not valid")
        return 0

    try:
        with open('./presets/'+ preset ,'r',encoding="utf-8") as file:
            json_data = json.load(file)
            send_json_hash(json_data)
            lastJSON_source = detect_source(json_data)
            #send the instruccion to start the selected choice
            _input = "action,"+"start"+"\x03"
            if(connection.port != None): connection.port.write(str.encode(_input))
    except:
        logger.info("Preset not found")
        return 0



#@sio.on('parameters') #To hardcode using send config

#data seems to not be used but is kept as optional to ease implementation if needed
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
    global sensor_status
    global data_sensors
    global sensor_sensors
    global esp_info

    #Variables to save data
    idle_in_data = False
    save_str = False
    
    shot_start_time = time.time()
    # global start_time

    connection.port.reset_input_buffer()
    connection.port.write(b'32\n')
    uart = ReadLine(connection.port)

    old_status = MachineStatusEnum.IDLE
    time_flag = False
    while not stopESPcomm:
        data = uart.readline()
        if len(data) > 0:
            # data_bit = bytes(data)
            try:
                data_str = data.decode('utf-8')
            except:
                logger.info(f"decoding fails, message: {data}")
                continue

            if 'Data' in data_str:
                if 'idle' in data_str:
                    idle_in_data = True
                    save_str = False
                else:
                    idle_in_data = False
                    save_str = True
            elif 'Sensors' in data_str:
                if idle_in_data:
                    save_str = False
                else:
                    save_str = True
            else:
                save_str = True

            if save_str or sensor_status:
                logger.info(data_str.strip("\r\n"))

            data_str_sensors = data_str.strip("\r\n").split(',')

            # potential message types
            button_event = None
            sensor = None
            data = None
            info = None

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
                is_idle = data.status == MachineStatusEnum.IDLE
                is_infusion = data.status == MachineStatusEnum.INFUSION
                is_preinfusion = data.status == MachineStatusEnum.PREINFUSION
                is_spring = data.status == MachineStatusEnum.SPRING
                was_preparing = old_status == MachineStatusEnum.CLOSING_VALE

                if (was_preparing and (is_preinfusion or is_infusion or is_spring)):
                    time_flag = True
                    shot_start_time = time.time()
                    logger.info("start_time: {:.1f}".format(shot_start_time))

                if (is_idle):
                    time_flag = False

                if (time_flag):
                    data_sensors = data.clone_with_time(time.time() - shot_start_time)
                else:
                    data_sensors = data
                old_status = data_sensors.status
                infoReady = True

            if sensor is not None:
                sensor_sensors = sensor
                infoReady = True

            if info is not None:
                esp_info = info
                infoReady = True

            if button_event is not None:
                logger.debug(f"Sending button event = {button_event.to_sio()}")
                await sio.emit("button", button_event.to_sio())

            # FIXME this should be callback to the frontends in the future
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

    # RotaryEncoder(down_switch,up_switch,menu_switch, lambda event: asyncio.run(tuner_event(event)))

#     with open('./json/profile.json') as json_file:
#         json_object = json.load(json_file)
#         json_file.close()

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

        if (reboot_flag): await sio.emit("MANUAL-REBOOT")

        await sio.emit("status", {**data_sensors.to_sio(), "source": lastJSON_source,})

        if sendInfoToFront:
            logger.info("Sending info to front")
            if sensor_sensors is not None:
                await sio.emit("sensors", sensor_sensors.to_sio_temperatures())
                await sio.emit("comunication", sensor_sensors.to_sio_communication())
                await sio.emit("actuators", sensor_sensors.to_sio_actuators())
            if esp_info is not None:
                # FIXME change to lowercase info for consistency
                await sio.emit("INFO", {**software_info, **esp_info.to_sio()})
        await sio.sleep(SAMPLE_TIME)
        i = i + 1

def send_data():
    global print_status
    print_status=True
    global sensor_status
    sensor_status=False
    global lastJSON_source
    global ble_gatt_server

    while (True):
        _input = input()

        if _input == "reset":
            connection.reset()

        elif _input == "show":
            print_status=True
            sensor_status=True
        
        elif _input == "hide":
            print_status=True
            sensor_status=False

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
            sensor_status=True
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
            sensor_status=False

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

        elif _input != "":
            logger.info(f"Unknown command: \"{_input}\"")
            pass
            # if _input[0] == "j" :
            #     _input = "json\n"+ _input +"\x03"s
            #     connection.port.write(str.encode(_input))
            # else:
            #     _input = "action,"+_input+"\x03"
            #     connection.port.write(str.encode(_input))
            
def main():
    global data_thread
    global send_data_thread
    global ble_gatt_server

    parse_command_line()

    gatherVersionInfo()

    ble_gatt_server = GATTServer.getServer()

    data_thread = threading.Thread(target=data_treatment)
    # data_thread.daemon = True
    data_thread.start()

    send_data_thread = threading.Thread(target=send_data) 
    # send_data_thread.daemon = True
    send_data_thread.start()

    ping_thread = threading.Thread(target=live_ping)
    ping_thread.start()

    watcher_listen_thread = threading.Thread(target=listen_watcher)
    watcher_listen_thread.start()
    
    app = tornado.web.Application(
        [
            (r"/socket.io/", socketio.get_tornado_handler(sio)),
        ],
        debug=options.debug,
    )

    app.listen(options.port)
    
    sio.start_background_task(live)
    tornado.ioloop.IOLoop.current().start()

def menu():    
    logger.info("Saludos, selecciona la opcion que deseas: ")
    logger.info("reset --> Al introducir esta opcion se reiniciara la esp32")
    logger.info("Acciones: tare, stop, start, purge, home   -----------> Haran las acciones correspondientes en la esp32")
    logger.info("json --> Al introducir esta opcion enviara el Json de nombre XXXXXX.XXXX contenido en la carpeta que contenga en codigo ")
    logger.info("show --> Muestra datos recibidos de la esp32")
    logger.info("hide --> Deja de mostrar datos recibidos de la esp32 exceptuando los mensajes del estado")
    logger.info("test --> Mueve el motor 10 veces de purge a home y muestra el valor de los sensores")
    logger.info("calibration --> Acceder a la funcion de la siguiente manera:  calibration,peso conocido,peso medido \n \t Ejemplo: calibration,100,90")

def listen_watcher():
    global pipe1
    try:
        pipe1 = os.open(pipe1_path, os.O_RDONLY | os.O_NONBLOCK)
    except OSError as e:
        logger.error(f'an error occurred oppening pipe1: {e}')
        pipe1 = None

    while True:
        if pipe1 != None:
            try:
                IPC_message = os.read(pipe1, 1024)
            except OSError as e:
                logger.error(f'error reading pipe1: {e}')
            if IPC_message:
                logger.debug("message receive from watcher:")
                if IPC_message.decode() == "FREE":
                    logger.debug("free resources")
                    startUpdate()

def startUpdate():
    global data_thread
    global stopESPcomm
    global connection

    stopESPcomm = True

    #stops the task that comunicates with the ESP
    if data_thread != None:
        data_thread.join()

    connection.sendUpdate()

#this function will ping the watcher that the back is live
def live_ping():
    while not stopESPcomm:
        logger.info("pinging watcher")
        with open(pipe2_path, 'w') as watcher:
            try:
                watcher.write("a")
                logger.info("watcher_pinged")
            except Exception as e:
                logger.info(f'watcher not pinged: {e}')
        time.sleep(1)

if __name__ == "__main__":

    USE_USB=os.getenv("USE_USB", 'False').lower() in ('true', '1', 'y')

    if USE_USB:
        connection = USBSerialConnection('/dev/ttyUSB0')
    else:
        # Default case in prod
        connection = FikaSerialConnection('/dev/ttymxc0')

    menu()
    connection.reset()
    try:
        main()
    except Exception as e:
        logger.exception("main() failed", exc_info=e, stack_info=True)