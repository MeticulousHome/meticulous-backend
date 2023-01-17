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
import RPi.GPIO as GPIO
from dotenv import load_dotenv
from datetime import datetime
import os
import os.path
from operator import itemgetter

comando = '/home/meticulous/meticulous-raspberry-setup/backend_for_esp32/clean_logs.sh'
lock = threading.Lock()
file_path = '/home/meticulous/meticulous-raspberry-setup/backend_for_esp32/logs/'
buffer=""
contador= 'contador.txt'

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

# load_dotenv()



on_off_bt = 18
lcd_en = 25
esp_en = 8
lcd_flt = 7
esp_flt = 12
    
if os.environ.get("PCB_VERSION") == "V3":
    en = 27
    io0 = 17
elif os.environ.get("PCB_VERSION") == "V3.1":
    en = 24
    io0 = 23
else:
    en = 24
    io0 = 23
    print("Set pines to V3.1") 
    
GPIO.setmode(GPIO.BCM)
GPIO.setup(en, GPIO.OUT)
GPIO.setup(io0, GPIO.OUT)
GPIO.setup(esp_en, GPIO.OUT)
GPIO.setup(lcd_en, GPIO.OUT)
GPIO.setup(on_off_bt, GPIO.IN)
GPIO.setup(lcd_flt, GPIO.IN)
GPIO.setup(esp_flt, GPIO.IN)

def turn_on():
    GPIO.output(esp_en, 1)
    GPIO.output(lcd_en, 1)

def turn_off():
    GPIO.output(esp_en, 0)
    GPIO.output(lcd_en, 0)
    
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
    "time": 0
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

def read_on_off_bt():
    time.sleep(0.1)
    return GPIO.input(on_off_bt)

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
    print("CLICK!")

def encoder_long_function():
    keyboard.press('e')
    keyboard.release('e')
    print("LONG ENCODER!")

def start_function():
    keyboard.press(Key.enter)
    keyboard.release(Key.enter)
    print("START!")

def reboot_esp():
    GPIO.output(en, 0)
    GPIO.output(io0, 0) 
    time.sleep(.1)
    GPIO.output(en, 1)
    GPIO.output(io0, 1)
    time.sleep(.1)
    GPIO.output(en, 0)
    time.sleep(.1)
    GPIO.output(en, 1)

@sio.event
def connect(sid, environ):
    print('connect ', sid)

@sio.event
def disconnect(sid):
    print('disconnect ', sid)

@sio.on('action')
def msg(sid, data):
    time.sleep(0.05)
    data = "action,"+data+"\x03"
    print(data)
    arduino.write(data.encode("utf-8"))

@sio.on('parameters')
def msg(sid, data):
    json_data = json.dumps(data, indent=1, sort_keys=False)
    json_data = "json\n"+json_data+"\x03"
    arduino.write(json_data.encode("utf-8"))

@sio.on('preset')
def msg(sid, data):
    # json_data = json.dumps(data, indent=1, sort_keys=False)
    # json_data = "json\n"+json_data+"\x03"
    # arduino.write(json_data.encode("utf-8"))
    if (data == "breville"):
        with open('./presets/breville.json','r',encoding="utf-8") as file:
            json_data = json.load(file)
            json_data = json.dumps(json_data, indent=1,sort_keys=False)
            json_data = "json\n"+json_data+"\x03"
            arduino.write(json_data.encode("utf-8"))
            _input = "action,"+"start"+"\x03"
            arduino.write(str.encode(_input))
    elif (data == "cube"):
        with open('./presets/cube.json','r',encoding="utf-8") as file:
            json_data = json.load(file)
            json_data = json.dumps(json_data, indent=1,sort_keys=False)
            json_data = "json\n"+json_data+"\x03"
            arduino.write(json_data.encode("utf-8"))
            _input = "action,"+"start"+"\x03"
            arduino.write(str.encode(_input))
    elif (data == "diletta"):
        with open('./presets/diletta.json','r',encoding="utf-8") as file:
            json_data = json.load(file)
            json_data = json.dumps(json_data, indent=1,sort_keys=False)
            json_data = "json\n"+json_data+"\x03"
            arduino.write(json_data.encode("utf-8"))
            _input = "action,"+"start"+"\x03"
            arduino.write(str.encode(_input))
    elif (data == "flair"):
        with open('./presets/flair.json','r',encoding="utf-8") as file:
            json_data = json.load(file)
            json_data = json.dumps(json_data, indent=1,sort_keys=False)
            json_data = "json\n"+json_data+"\x03"
            arduino.write(json_data.encode("utf-8"))
            _input = "action,"+"start"+"\x03"
            arduino.write(str.encode(_input))
    elif (data == "la-pavoni"):
        with open('./presets/la-pavoni.json','r',encoding="utf-8") as file:
            json_data = json.load(file)
            json_data = json.dumps(json_data, indent=1,sort_keys=False)
            json_data = "json\n"+json_data+"\x03"
            arduino.write(json_data.encode("utf-8"))
            _input = "action,"+"start"+"\x03"
            arduino.write(str.encode(_input))
    elif (data == "rocket"):
        with open('./presets/rocket.json','r',encoding="utf-8") as file:
            json_data = json.load(file)
            json_data = json.dumps(json_data, indent=1,sort_keys=False)
            json_data = "json\n"+json_data+"\x03"
            arduino.write(json_data.encode("utf-8"))
            _input = "action,"+"start"+"\x03"
            arduino.write(str.encode(_input))
    else:
        print("Preset not valid")







# arduino = serial.Serial("COM4",115200)
arduino = serial.Serial('/dev/ttyS0',115200)

def add_to_buffer(message_to_save):
    global buffer
    global lock
    with lock:
        buffer = buffer + message_to_save

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
            if "Sensors" not in data_str:
                if "idle" in data_str:
                    flag_idle=False
                else:
                    flag_idle=True
            if "idle" not in data_str and flag_idle==True:
                current_date_time = datetime.now().strftime("%Y_%m_%d %H:%M:%S.%f, ")
                add_to_buffer(current_date_time)
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

                c1 = old_status == "heating"
                c2 = data_sensors["status"] == "preinfusion"
                # print(c1, end = "")
                # print(c2, end = "")
                # print(len(data_sensors["status"]), end = "")

                # time = time.time() - start_time
                if (c1 and c2):
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
            elif data_str.find("CCW") > -1:
                ccw_function()
            elif data_str.find("CW") > -1:
                cw_function()
            elif data_str.find("push") > -1:
                encoder_push_function()
            elif data_str.find("elng") > -1:
                encoder_long_function()
            elif data_str.find("ta_d") > -1:
                tare_double_function()
            elif data_str.find("ta_l") > -1:
                tare_long_function()
            elif data_str.find("strt") > -1:
                start_function()
            else:
                if print_status==True:
                    if sensor_status==True:
                        print(data_str)
                    
                    else:
                        if data_str[0]=="E":
                            print(data_str)
                        else:
                            pass
                else:
                    pass

# print(data_str_sensors[0])
# print(data_str)
    
def data_treatment():
    read_arduino()

async def live():

    global coffee_status

    # RotaryEncoder(down_switch,up_switch,menu_switch, lambda event: asyncio.run(tuner_event(event)))

#     with open('./json/profile.json') as json_file:
#         json_object = json.load(json_file)
#         json_file.close()

    process_started = False
    SAMPLE_TIME = 0.1
    elapsed_time = 0
    i = 0
    time = 0
    while True:
        await sio.emit("status", {
            "name": data_sensors["status"],
            # "name" : "idle",
            "sensors": {
                "p": data_sensors["pressure"],
                "f": data_sensors["flow"],
                "w": data_sensors["weight"],
                "t": data_sensors["temperature"],
            },
            "time": str(data_sensors["time"])
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
            json_data = "json\n"+json_data+"\x03"
            # arduino.write(json_data.encode("utf-8"))
            arduino.write(str.encode(json_data))
            json_data=""
            json_file=""
            

        elif _input=="tare" or _input=="stop" or _input=="purge" or _input=="home" or _input=="start" :
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
    

if __name__ == "__main__":
    os.system(comando)

    date = datetime.now().strftime("%Y_%m_%d")  

    # # recorrer todos los archivos en el directorio
    # files = []
    # for file in os.scandir(file_path):
    #         if file.is_file():
    #             files.append((file, os.path.getmtime(file)))
    # # ordenar los archivos por fecha de modificación
    # files.sort(key=itemgetter(1), reverse=True)
    # # obtener el nombre del último archivo modificado
    # last_modified_file = files[0][0].name
    # last_date=last_modified_file[5:15]

    # if date!=last_date:
    #     flag_fecha_distinta=True
    # else:
    #     flag_fecha_distinta=False
    with open(file_path + contador, 'a+', newline='') as file:
        pass

    with open(file_path + contador, 'r+', newline='') as file:
        first_line = file.readline()
        if first_line == '' :
            session_number=1
            file.write(str(1))
        else:
            value = int(first_line)
            value = value + 1
            session_number = value
            file.seek(0)
            file.write(str(value))
    
    file_name = 'Fika_' + date +'_'+ str(session_number) + '.txt' 
    menu()
    reboot_esp()
    try:
        main()
        
    except:
        traceback.print_exc()
        GPIO.cleanup()
