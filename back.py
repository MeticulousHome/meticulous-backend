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
from pynput.keyboard import Key, Controller
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

from esp_serial.connection.fika_serial_connection import FikaSerialConnection

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

autoupdate_path = "./meticulous-raspberry-setup/meticulous-autoupdate"
user_path=os.path.expanduser("~/")

usaFormatoDeColores = True

sendInfoToFront = False

infoReady = False

lastJSON_source = "LCD"

reboot_flag = False

connection = None

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

def createUpdateDir():
    # Specify the directory path you want to create
    directory_path = os.path.expanduser("~/update")

    # Check if the directory already exists
    if not os.path.exists(directory_path):
    # Create the directory if it does not exist
        os.makedirs(directory_path)
        #logger.info(f"Directory '{directory_path}' created successfully.")

keyboard = Controller()

define("port", default=8080, help="run on the given port", type=int)
define("debug", default=False, help="run in debug mode")

sio = socketio.AsyncServer(cors_allowed_origins='*', async_mode='tornado')

data_sensors = {
    "pressure":1,
    "flow":2,
    "weight":3,
    "temperature":4,
    "status": "idle",
    "time": 0,
    "profile": "idle"
}

data_sensor_temperatures = {
    "external_1":1,
    "external_2":2,
    "bar_up":3,
    "bar_mid_up":4,
    "bar_mid_down": 5,
    "bar_down": 6,
    "tube": 7,
    "valve": 8
}

data_sensor_comunication = {
    "preassure_sensor": 1,
    "adc_0": 2,
    "adc_1": 3,
    "adc_2": 4,
    "adc_3": 5
}

data_sensor_actuators = {
    "motor_position": 1,
    "motor_speed": 2,
    "motor_power": 3,
    "motor_current": 4,
    "bandheater_power": 5
}

software_info = {
    "name": "Meticulous Espresso",
    "lcdV": 3,
    "firmwareV": 4,
    "backendV": 5,
    "fanStatus": "on",
}

hardware_info = {
    "mainVoltage": "240"
}

# "d" -> double click tare
# "s" -> long tare
# "x" -> double click encoder
# "e" -> long click encoder
# "enter" -> click boton principal

def cw_function():
    keyboard.press(Key.right)
    keyboard.release(Key.right)
    logger.info("RIGHT!")

def ccw_function():
    keyboard.press(Key.left)
    keyboard.release(Key.left)
    logger.info("LEFT!")

def tare_double_function():
    keyboard.press('d')
    keyboard.release('d')
    logger.info("DOUBLE TARE!")

def tare_long_function():
    keyboard.press('s')
    keyboard.release('s')
    logger.info("LONG TARE!")

def encoder_push_function():
    keyboard.press(Key.space)
    keyboard.release(Key.space)
    logger.info("PUSH ENCODER!")

def encoder_double_function():
    keyboard.press('x')
    keyboard.release('x')
    if (data_sensors["status"] != "idle"):
        _input = "action,"+"stop"+"\x03"
        if(connection.port != None): connection.port.write(str.encode(_input))
    logger.info("DOUBLE ENCODER!")

def encoder_long_function():
    start_function()
    logger.info("LONG ENCODER!")

def start_function():
    keyboard.press(Key.enter)
    keyboard.release(Key.enter)
    logger.info("START!")

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
def setSendInfo(sid):
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
    current_weight = data_sensors["weight"]
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

def read_arduino():
    global infoReady
    global stopESPcomm
    global sensor_status

    #Variables to save data
    idle_in_data = False
    save_str = False
    
    start_time = time.time()
    # global start_time

    connection.port.reset_input_buffer()
    connection.port.write(b'32\n')
    uart = ReadLine(connection.port)

    old_status = ""
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

            data_str_sensors = data_str.split(',')

            if data_str_sensors[0] == 'Data':
                data_sensors["pressure"] = data_str_sensors[1]
                data_sensors["flow"]= data_str_sensors[2]
                data_sensors["weight"] = data_str_sensors[3]
                data_sensors["temperature"] = data_str_sensors[4]
                status_bad = data_str_sensors[5]
                data_sensors["status"] = status_bad.strip("\n")
                data_sensors["status"] = data_sensors["status"].strip("\r")

                try:
                    data_sensors["profile"] = data_str_sensors[6].strip("\n")
                    data_sensors["profile"] = data_sensors["profile"].strip("\r")
                except:
                    data_sensors["profile"] = "None"

                c1 = old_status == "closing valve"
                c2 = data_sensors["status"] == "preinfusion"
                c3 = data_sensors["status"] == "infusion"
                c4 = data_sensors["status"] == "spring"
                # logger.info(c1, end = "")
                # logger.info(c2, end = "")
                # logger.info(len(data_sensors["status"]), end = "")

                # time = time.time() - start_time
                if ((c1 and c2) or (c1 and c3) or (c1 and c4)):
                    time_flag = True
                    start_time = time.time()
                    logger.info("start_time: {:.1f}".format(start_time))
                if (data_sensors["status"] == "idle"):
                    time_flag = False

                if (time_flag):
                    data_sensors["time"] = time.time() - start_time
                else:
                    data_sensors["time"] = 0

                old_status = data_sensors["status"]
                # logger.info(data_sensors["status"])

                #else:
                #    para cuando no usa formato de colores
                
            elif data_str.find("CCW") > -1:
                ccw_function()
            elif data_str.find("CW") > -1:
                cw_function()
            elif data_str.find("push") > -1:
                encoder_push_function()
            elif data_str.find("pu_d") > -1:
                encoder_double_function()
            elif data_str.find("elng") > -1:
                encoder_long_function()
            elif data_str.find("ta_d") > -1:
                tare_double_function()
            elif data_str.find("ta_l") > -1:
                tare_long_function()
            elif data_str.find("strt") > -1:
                start_function()

            elif data_str_sensors[0] == 'Sensors':
                if usaFormatoDeColores:
                    try:
                        sensor_values = data_str_sensors[1].split('\033[0m')
                        data_sensor_temperatures["external_1"] = sensor_values[1].split('\033[1;31m')[0]
                        data_sensor_temperatures["external_2"] = sensor_values[2].split('\033[1;32m')[0]
                        data_sensor_temperatures["bar_up"] = sensor_values[3].split('\033[1;32m')[0]
                        data_sensor_temperatures["bar_mid_up"] = sensor_values[4].split('\033[1;32m')[0]
                        data_sensor_temperatures["bar_mid_down"] = sensor_values[5].split('\033[1;32m')[0]
                        data_sensor_temperatures["bar_down"] = sensor_values[6].split('\033[1;32m')[0]
                        data_sensor_temperatures["tube"] = sensor_values[7].split('\033[1;33m')[0]
                        data_sensor_temperatures["valve"] = sensor_values[8].split('\033[1;34m')[0]
                        data_sensor_actuators["motor_position"]=sensor_values[9].split('\033[1;34m')[0]
                        data_sensor_actuators["motor_speed"]=sensor_values[10].split('\033[1;36m')[0]
                        data_sensor_actuators["motor_power"]=sensor_values[11].split('\033[1;36m')[0]
                        data_sensor_actuators["motor_current"]=sensor_values[12].split('\033[1;36m')[0]
                        data_sensor_actuators["bandheater_power"]=sensor_values[13].split('\033[1;35m')[0]
                        data_sensor_comunication["preassure_sensor"] = sensor_values[14].split('\033[1;35m')[0]
                        data_sensor_comunication["adc_0"] = sensor_values[15].split('\033[1;35m')[0]
                        data_sensor_comunication["adc_1"] = sensor_values[16].split('\033[1;35m')[0]
                        data_sensor_comunication["adc_2"] = sensor_values[17].split('\033[1;35m')[0]
                        data_sensor_comunication["adc_3"] = sensor_values[18].split('\n')[0]
                    except:
                        # pass
                        logger.error("ESP did not send sensor values correctly")

            elif data_str_sensors[0] == 'ESPInfo':
                info_not_valid = False

                try:
                    software_info["firmwareV"] = data_str_sensors[1]
                except:
                    software_info["firmwareV"]  = "not found"
                    logger.error("ESP did not send firmware version correctly\n")
                    info_not_valid = True
                
                try:
                    software_info["fanStatus"] = data_str_sensors[2]
                except:
                    logger.error("ESP did not send fanStatus correctly")
                    info_not_valid = True

                try:
                    hardware_info["mainVoltage"] = data_str_sensors[3].strip('\r\n')
                except:
                    logger.error("ESP did not send main voltage value correctly")
                    info_not_valid = True
                
                infoReady = not info_not_valid      #if the info received is valid, then fla info ready, else dont flag it

# logger.info(data_str_sensors[0])
# logger.info(data_str)
    
def data_treatment():
    global connection
    if(connection.port != None): read_arduino()

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

        await sio.emit("status", {
            "name": data_sensors["status"],
            # "name" : "idle",
            "sensors": {
                "p": data_sensors["pressure"],
                "f": data_sensors["flow"],
                "w": data_sensors["weight"],
                "t": data_sensors["temperature"],
            },
            "time": str(data_sensors["time"]),
            "profile": data_sensors["profile"],
            "source": lastJSON_source,
        })

        if sendInfoToFront:
            await sio.emit("sensors", {
                "t_ext_1": data_sensor_temperatures["external_1"],
                "t_ext_2": data_sensor_temperatures["external_2"],
                "t_bar_up": data_sensor_temperatures["bar_up"],
                "t_bar_mu": data_sensor_temperatures["bar_mid_up"],
                "t_bar_md": data_sensor_temperatures["bar_mid_down"],
                "t_bar_down": data_sensor_temperatures["bar_down"],
                "t_tube": data_sensor_temperatures["tube"],
                "t_valv": data_sensor_temperatures["valve"],
            })
            
            await sio.emit("comunication", {
                "p": data_sensor_comunication["preassure_sensor"],
                "a_0": data_sensor_comunication["adc_0"],
                "a_1": data_sensor_comunication["adc_1"],
                "a_2": data_sensor_comunication["adc_2"],
                "a_3": data_sensor_comunication["adc_3"]
            })
    
            await sio.emit("actuators", {
                "m_pos": data_sensor_actuators["motor_position"],
                "m_spd": data_sensor_actuators["motor_speed"],
                "m_pwr": data_sensor_actuators["motor_power"],
                "m_cur": data_sensor_actuators["motor_current"],
                "bh_pwr": data_sensor_actuators["bandheater_power"]
            })
    
            await sio.emit("INFO", {
                "name": software_info["name"],
                "lcdV" : software_info["lcdV"],
                "firmwareV" : software_info["firmwareV"],
                "backendV" : software_info["backendV"],
                "fanStatus": software_info["fanStatus"],
                "mainVoltage": hardware_info["mainVoltage"],
            })
        await sio.sleep(SAMPLE_TIME)
        i = i + 1

def send_data():
    global print_status
    print_status=True
    global sensor_status
    sensor_status=False
    global lastJSON_source
    
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

        else:
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
    
    parse_command_line()

    gatherVersionInfo()

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

    if(connection.port != None): connection.port.close()

    time.sleep(1)

    # TELL WATCHER RESOURCES ARE FREE
    with open(pipe2_path, 'w') as pipe:
        try:
            pipe.write("released")
        except:
            logger.error("error writing to watcher")

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

    connection = FikaSerialConnection('/dev/ttymxc0')

    menu()
    connection.reset()
    try:
        main()
    except Exception as e:
        logger.exception("main() failed", exc_info=e, stack_info=True)