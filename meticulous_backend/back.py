from tornado.options import define, options, parse_command_line
import socketio
import tornado.web
import tornado.ioloop
import traceback
import serial
import threading
import time
import json
from pynput.keyboard import Key, Controller
from dotenv import load_dotenv
from datetime import datetime
import os
import os.path
from operator import itemgetter
import hashlib
import version as backend
import subprocess
import gpiod

base_dir = os.path.dirname(os.path.abspath(__file__))


comando = os.path.join(base_dir, 'clean_logs.sh')
lock = threading.Lock()
file_path = os.path.join(base_dir, 'logs')
buffer=""
contador= 'contador.txt'

usaFormatoDeColores = True

sendInfoToFront = False

infoReady = False

#VERSION INFORMATION

borrarFormato = "\033[0m"
colores = [
    "\033[1;31m", # Rojo
    "\033[1;32m", # Verde
    "\033[1;33m", # Amarillo
    "\033[1;34m", # Azul
    "\033[1;35m", # Morado
    "\033[1;36m", # Cian
]

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
        while True:
            i = max(1, min(2048, self.s.in_waiting))
            data = self.s.read(i)
            i = data.find(b"\n")
            if i >= 0:
                r = self.buf + data[:i+1]
                self.buf[0:] = data[i+1:]
                return r
            else:
                self.buf.extend(data)

load_dotenv()


lcd_en = 25
esp_en = 8

chip_number = 0
chip = gpiod.chip(chip_number)

    
if os.environ.get("PINES_VERSION") == "V3":
    en = 27
    io0 = 17
    print("Set pines to V3") 
elif os.environ.get("PINES_VERSION") == "V3.1":
    en = 24
    io0 = 23
    print("Set pines to V3.1") 
else:
    en = 24
    io0 = 23
    print("Set pines to V3.1") 
    
    lines = chip.get_lines([en, io0, esp_en, lcd_en])
    config = gpiod.line_request()
    config.consumer = "meticulos-backend"
    config.request_type = gpiod.line_request.DIRECTION_OUTPUT
    lines.request(config)

def gatherVersionInfo():
    global infoSolicited
    software_info["name"] = "Meticulous Espresso"
    software_info["backendV"] = backend.VERSION

    #OBTENEMOS EL NOMBRE DE LA APLICACION DE LA LCD
    auxFile = open(os.path.expanduser("~/.xsession"))
    lcd_ui_name = auxFile.read().split('\n')[2].split()[1]

    #OBTENEMOS SU VERSION USANDO LOS COMANDOS DPKG y GREP
    command = f'dpkg --list | grep {lcd_ui_name}'
    result = subprocess.run(command, shell=True, capture_output=True, text=True)
    lcd_version = result.stdout.split()[2]

    infoSolicited = True

    software_info["lcdV"] = lcd_version

    software_info["dashboardV"] = 1.0
    software_info["firmwareV"] = 0.0

    #SOLICITAMOS LA VERSION DE FIRMWARE A LA ESP

def turn_on():
    if os.environ.get("EN_PIN_HIGH") == "0":
        lines.get(esp_en).set_value(0)
        lines.get(lcd_en).set_value(0)
        print("EN_PIN_HIGH = 0")
    elif os.environ.get("EN_PIN_HIGH") == "1":
        lines.get(esp_en).set_value(1)
        lines.get(lcd_en).set_value(1)
        print("EN_PIN_HIGH = 1")
    else:
        lines.get(esp_en).set_value(0)
        lines.get(lcd_en).set_value(0)
        print("EN_PIN_HIGH = 0 por default")

def turn_off():
    if os.environ.get("EN_PIN_HIGH") == "0":
        lines.get(esp_en).set_value(1)
        lines.get(lcd_en).set_value(1)
        print("EN_PIN_HIGH = 0")
    elif os.environ.get("EN_PIN_HIGH") == "1":
        lines.get(esp_en).set_value(0)
        lines.get(lcd_en).set_value(0)
        print("EN_PIN_HIGH = 1")
    else:
        lines.get(esp_en).set_value(1)
        lines.get(lcd_en).set_value(1)
        print("EN_PIN_HIGH = 0 por default")
    
turn_on()
#os.system('killall coffee-ui-demo')
#time.sleep(5)

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
    "dashboardV": 2,
    "lcdV": 3,
    "firmwareV": 4,
    "backendV": 5,
    "fanStatus": "on",
}

# "d" -> double click tare
# "s" -> long tare
# "x" -> double click encoder
# "e" -> long click encoder
# "enter" -> click boton principal

def enable_pcb():
    global keyboard
    while True:
            turn_on()

def cw_function():
    keyboard.press(Key.right)
    keyboard.release(Key.right)
    print("RIGHT!")

def ccw_function():
    keyboard.press(Key.left)
    keyboard.release(Key.left)
    print("LEFT!")

def tare_double_function():
    keyboard.press('d')
    keyboard.release('d')
    print("DOUBLE TARE!")

def tare_long_function():
    keyboard.press('s')
    keyboard.release('s')
    print("LONG TARE!")

def encoder_push_function():
    keyboard.press(Key.space)
    keyboard.release(Key.space)
    print("PUSH ENCODER!")

def encoder_double_function():
    keyboard.press('x')
    keyboard.release('x')
    if (data_sensors["status"] != "idle"):
        _input = "action,"+"stop"+"\x03"
        arduino.write(str.encode(_input))
    print("DOUBLE ENCODER!")

def encoder_long_function():
    start_function()
    print("LONG ENCODER!")

def start_function():
    keyboard.press(Key.enter)
    keyboard.release(Key.enter)
    print("START!")

def reboot_esp():
    lines.get(esp_en).set_value(0)
    lines.get(io0).set_value(0)
    time.sleep(.1)
    lines.get(en).set_value(1)
    lines.get(io0).set_value(1)
    time.sleep(.1)
    lines.get(en).set_value(0)
    time.sleep(.1)
    lines.get(en).set_value(1)

def send_json_hash(json_string):
    json_data = "json\n"+json_string+"\x03"
    add_to_buffer(json_data)
    # print(json_data)
    json_hash = hashlib.md5(json_data[5:-1].encode('utf-8')).hexdigest()
    add_to_buffer("hash_enviado: " + json_hash + "\n")
    print("hash: ",end="")
    print(json_hash)
    arduino.write("hash ".encode("utf-8"))
    arduino.write(json_hash.encode("utf-8"))
    arduino.write("\x03".encode("utf-8"))
    arduino.write(json_data.encode("utf-8"))

@sio.event
def connect(sid, environ):
    print('connect ', sid)

@sio.event
def disconnect(sid):
    print('disconnect ', sid)
sendInfoToFront
@sio.on('action')
def msg(sid, data):
    if data == "start":
        time.sleep(0.5)
        data = "action,"+data+"\x03"
        print(data)
        arduino.write(data.encode("utf-8"))
    else:
        time.sleep(0.05)
        data = "action,"+data+"\x03"
        print(data)
        arduino.write(data.encode("utf-8"))

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
        print("fans on")
        _solicitud ="action,fans-on\x03"
    else:
        print("fans off")
        _solicitud = "action,fans-off\x03"
    arduino.write(str.encode(_solicitud))
    software_info["fanStatus"] = 'on' if data else 'off'

@sio.on('parameters')
def msg(sid, data):
    json_data = json.dumps(data, indent=1, sort_keys=False)
    send_json_hash(json_data)

@sio.on('preset')
def msg(sid, data):
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
        print("Preset not valid")
        return 0

    try:
        with open('./presets/'+ preset ,'r',encoding="utf-8") as file:
            json_data = json.load(file)
            json_data = json.dumps(json_data, indent=1,sort_keys=False)
            send_json_hash(json_data)
            #send the instruccion to start the selected choice
            _input = "action,"+"start"+"\x03"
            arduino.write(str.encode(_input))
    except:
        print("Preset not found")
        return 0



#@sio.on('parameters') #To hardcode using send config
@sio.on('calibration') #Use when calibration it is implemented
def msg(sid, data):
    know_weight = "100.0"
    current_weight = data_sensors["weight"]
    data ="calibration"+","+know_weight+","+str(current_weight)
    _input = "action,"+data+"\x03"
    arduino.write(str.encode(_input))




# arduino = serial.Serial("COM4",115200)
# arduino = serial.Serial('/dev/ttyS0',115200)
# arduino = serial.Serial('/dev/ttyUSB0',115200)
def detect_arduino_port():
    # Try opening /dev/ttyS0 and /dev/ttyUSB0
    reboot_esp()
    for port in ['/dev/ttyS0', '/dev/ttyUSB0']:
        try:
            ser = serial.Serial(port, baudrate=115200, timeout=1)
            time.sleep(2)
            # Wait for incoming data
            incoming_data = ser.readline()
            ser.close()
            # If there was incoming data, return the serial port
            if incoming_data:
                return port
        except (OSError, serial.SerialException):
            print("Serial Exception raised")
    # If no Arduino was detected, return None
    return None

def add_to_buffer(message_to_save):
    global buffer
    global lock
    current_date_time = datetime.now().strftime("%Y_%m_%d %H:%M:%S.%f, ")
    with lock:
        buffer = buffer + current_date_time + message_to_save

def save_log():
    global file_name
    global file_path
    global lock
    global buffer
    #start_time = time.time()
    with lock:
        with open(file_path + file_name, 'a+', newline='') as file:
            # current_date_time = datetime.now().strftime("%Y_%m_%d %H:%M:%S.%f, ")
            # file.write(current_date_time)
            file.write(buffer)
    #end_time = time.time()
    #elapsed_time = end_time - start_time
    #print("El tiempo de escritura fue de {} segundos".format(elapsed_time))  

def log():
    global buffer
    while True:
        if buffer!="":
            save_log()
            buffer=""
        else:
            pass
        time.sleep(5)

def read_arduino():
    global infoReady

    #Variables to save data
    idle_in_data = False
    save_str = False
    
    start_time = time.time()
    # global start_time

    # arduino = serial.Serial("COM3",115200)
    arduino.reset_input_buffer()
    arduino.write(b'32\n')
    uart = ReadLine(arduino)

    old_status = ""
    time_flag = False
    while True:
        data = uart.readline()
        if len(data) > 0:
            # data_bit = bytes(data)
            try:
                data_str = data.decode('utf-8')
            except:
                print("decoding fails, message: ", end=' ')
                print(data)
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

            if save_str:
                add_to_buffer(data_str)
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

                c1 = old_status == "heating"
                c2 = data_sensors["status"] == "preinfusion"
                c3 = data_sensors["status"] == "infusion"
                # print(c1, end = "")
                # print(c2, end = "")
                # print(len(data_sensors["status"]), end = "")

                # time = time.time() - start_time
                if ((c1 and c2) or (c1 and c3)):
                    time_flag = True
                    start_time = time.time()
                    print("start_time: {:.1f}".format(start_time))
                if (data_sensors["status"] == "idle"):
                    time_flag = False

                if (time_flag):
                    data_sensors["time"] = time.time() - start_time
                else:
                    data_sensors["time"] = 0

                old_status = data_sensors["status"]
                # print(data_sensors["status"])

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

                    if sensor_status:
                        print(data_str, end="")

            elif data_str_sensors[0] == 'ESPInfo':

                try:
                    software_info["fanStatus"] = data_str_sensors[2].strip('\r\n')
                except:
                    add_to_buffer("(E): ESP did not send fanStatus correctly")

                try:
                    software_info["firmwareV"] = data_str_sensors[1]
                except:
                    software_info["firmwareV"]  = "not found"
                    add_to_buffer("(E): ESP did not send firmware version correctly")

                infoReady = True

            elif print_status:
                    print(data_str)
            

# print(data_str_sensors[0])
# print(data_str)
    
def data_treatment():
    read_arduino()

async def live():

    global coffee_status
    global sendInfoToFront
    global infoReady
    global infoSolicited

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
            arduino.write(str.encode(_solicitud))

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
            "profile": data_sensors["profile"]
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
                "dashboardV" : software_info["dashboardV"],
                "lcdV" : software_info["lcdV"],
                "firmwareV" : software_info["firmwareV"],
                "backendV" : software_info["backendV"],
                "fanStatus": software_info["fanStatus"],
            })
        await sio.sleep(SAMPLE_TIME)
        i = i + 1

def send_data():
    global print_status
    print_status=True
    global sensor_status
    sensor_status=False

    while (True):
        _input = input()

        if _input == "reset":
            tr = threading.Thread(target=reboot_esp)
            tr.deamon = True
            tr.start()
        elif _input == "show":
            print_status=True
            sensor_status=True
        
        elif _input == "hide":
            print_status=True
            sensor_status=False

        elif _input== "json":
            with open('fika.json','r') as openfile:
                json_file = json.load(openfile)
            json_data = json.dumps(json_file, indent=1, sort_keys=False)
            send_json_hash(json_data)
            json_data=""
            json_file=""
            

        elif _input=="tare" or _input=="stop" or _input=="purge" or _input=="home" or _input=="start" :
            _input = "action,"+_input+"\x03"
            arduino.write(str.encode(_input))
            
        elif _input == "test":
            sensor_status=True
            for i in range(0,10):
                _input = "action,"+"purge"+"\x03"
                arduino.write(str.encode(_input))
                time.sleep(15)
                print(_input)
                _input = "action,"+"home"+"\x03"
                arduino.write(str.encode(_input))
                time.sleep(15)
                contador = "Numero de prueba: "+str(i+1)
                print(_input)
                print(contador)
            sensor_status=False

        elif _input[:11] == "calibration":
             _input = "action,"+_input+"\x03"
             arduino.write(str.encode(_input))

        else:
            pass
            # if _input[0] == "j" :
            #     _input = "json\n"+ _input +"\x03"s
            #     arduino.write(str.encode(_input))
            # else:
            #     _input = "action,"+_input+"\x03"
            #     arduino.write(str.encode(_input))
            
def main():

    
    parse_command_line()

    gatherVersionInfo()

    data_thread = threading.Thread(target=data_treatment)
    # data_thread.daemon = True
    data_thread.start()

    send_data_thread = threading.Thread(target=send_data) 
    # send_data_thread.daemon = True
    send_data_thread.start()

    log_thread=threading.Thread(target=log)
    log_thread.start()
    
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
    print("Saludos, selecciona la opcion que deseas: ")
    print("reset --> Al introducir esta opcion se reiniciara la esp32")
    print("Acciones: tare, stop, start, purge, home   -----------> Haran las acciones correspondientes en la esp32")
    print("json --> Al introducir esta opcion enviara el Json de nombre XXXXXX.XXXX contenido en la carpeta que contenga en codigo ")
    print("show --> Muestra datos recibidos de la esp32")
    print("hide --> Deja de mostrar datos recibidos de la esp32 exceptuando los mensajes del estado")
    print("test --> Mueve el motor 10 veces de purge a home y muestra el valor de los sensores")
    print("calibration --> Acceder a la funcion de la siguiente manera:  calibration,peso conocido,peso medido \n \t Ejemplo: calibration,100,90")
    

if __name__ == "__main__":

    # Call the function to get the port
    arduino_port = detect_arduino_port()

    # Open the serial connection if an Arduino was detected
    if arduino_port == '/dev/ttyS0':
        arduino = serial.Serial('/dev/ttyS0',115200)
        print("Serial connection opened on port ttyS0")
    elif arduino_port == '/dev/ttyUSB0':
        arduino = serial.Serial('/dev/ttyUSB0',115200)
        print("Serial connection opened on port ttyUSB0")
    else:
        print("No ESP32 available")

    # arduino = serial.Serial('/dev/ttyS0',115200)
    # arduino = serial.Serial('/dev/ttyUSB0',115200)

    os.system(comando) #Crea la carpeta donde se guardaran los datos 
    date = datetime.now().strftime("%Y_%m_%d") #Fecha actual

    try: #procesp para obtener el numero de sesion 9999 si no se puede obtener el numero de sesion
        with open(file_path + contador, 'a', newline='') as file: #Crea el archivo donde se guardara el numero de sesion si no existe
            pass

        with open(file_path + contador, 'r', newline='') as file: #Abre el archivo donde se guardara el numero de sesion
            first_line = file.readline() #Lee la primera linea del archivo
        if first_line == '': #Si el archivo esta vacio se crea el archivo con el numero de sesion 1
            session_number = 1 #asigna el numero de sesion 1 pues creo el archivo
            with open(file_path + contador, 'w', newline='') as file:
                file.write(str(1)) #Escribe el numero de sesion 1 en el archivo
        else: #Si el archivo no esta vacio se lee el numero de sesion y se le suma 1
            try: #procesp para obtener el numero de sesion 9999 si no se puede obtener el numero de sesion
                value = int(first_line)  
                value = value + 1 
                session_number = value
                with open(file_path + contador, 'w', newline='') as file:
                    file.write(str(value)) #Escribe el numero de sesion en el archivo
            except ValueError:
                print("Error, el contenido del archivo no es un número válido")
                session_number = 999 
                with open(file_path + contador, 'w', newline='') as file:
                    file.write(str(999))
            except:
                print("Error desconocido")
                session_number = 999
                with open(file_path + contador, 'w', newline='') as file:
                    file.write(str(999))    
    except:
        print("Error al abrir el archivo")
        session_number = 9999
    
    file_name = 'Fika_' + date +'_'+ str(session_number) + '.txt' 
    menu()
    reboot_esp()
    try:
        main()
        
    except:
        traceback.print_exc()
        chip.reset()
