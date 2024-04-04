from modes.italian_1_0.italian_1_0 import generate_italian_1_0
from modes.dashboard_1_0.dashboard_1_0 import generate_dashboard_1_0
from tornado.options import define, options, parse_command_line
import socketio
import tornado.log
import tornado.web
import tornado.ioloop
import threading
import time
import json
import os
import os.path
import version as backend
import subprocess
import asyncio

from esp_serial.data import ButtonEventData

from ble_gatt import GATTServer
from wifi import WifiManager
from notifications import Notification, NotificationManager, NotificationResponse
from profile import ProfileManager
from config import *
from machine import Machine
from sounds import SoundPlayer

from api.api import API
from api.emulation import register_emulation_handlers
from api.web_ui import WEB_UI_HANDLER

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

tornado.log.access_log = MeticulousLogger.getLogger("tornado.access")
tornado.log.app_log = MeticulousLogger.getLogger("tornado.application")
tornado.log.gen_log = MeticulousLogger.getLogger("tornado.general")

sendInfoToFront = True

PORT = int(os.getenv("PORT", '8080'))
DEBUG=os.getenv("DEBUG", 'False').lower() in ('true', '1', 'y')

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

sio = socketio.AsyncServer(cors_allowed_origins='*', async_mode='tornado')

software_info = {
    "name": "Meticulous Espresso",
    "lcdV": 3,
}

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
        Machine.write(data.encode("utf-8"))
    else:
        time.sleep(0.05)
        data = "action,"+data+"\x03"
        logger.info(data)
        Machine.write(data.encode("utf-8"))

@sio.on('askForInfo')
def setSendInfo(sid):
    global sendInfoToFront
    sendInfoToFront = True

@sio.on('notification')
def msg(sid, noti_json):
    notification = json.loads(noti_json)
    if "id" in notification and "response" in notification:
        NotificationManager.acknowledge_notification(notification["id"], notification["response"])


@sio.on('stopInfo')
def StopInfo(sid):
    global sendInfoToFront
    sendInfoToFront = False

@sio.on('send_profile')
async def forwardJSON(sid,data):
    logger.info(json.dumps(data, indent=1, sort_keys=False))
    await sio.emit('save_profile', data)

@sio.on('calibrate') #Use when calibration it is implemented
def msg(sid, data=True):
    know_weight = "100.0"
    current_weight = Machine.data_sensors.weight
    data ="calibration"+","+know_weight+","+str(current_weight)
    _input = "action,"+data+"\x03"
    Machine.write(str.encode(_input))

@sio.on('feed_profile')
async def feed_profile(sid, data):
    logger.info(f"Received JSON: {data}")  # Print the received JSON data
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
            Machine.send_json_with_hash(obj_json)
            await asyncio.sleep(5)
            _input = "action,"+"start"+"\x03"
            Machine.write(str.encode(_input))
            
        if kind_value =="dashboard_1_0":
            logger.info("Is Dashboard 1.0")
            action_value = obj.get('action', None)
            if action_value == "to_play":
                json_result = generate_dashboard_1_0(obj)  #<class 'str'>
                logger.info(json_result)
                obj_json = json.loads(json_result) #<class 'dict'>
                Machine.send_json_with_hash(obj_json)
                await asyncio.sleep(5)
                _input = "action,"+"start"+"\x03"
                Machine.write(str.encode(_input))
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

send_data_thread = None

async def live():

    global sendInfoToFront

    process_started = False
    SAMPLE_TIME = 0.1
    elapsed_time = 0
    i = 0
    _time = time.time()
    logger.info("Starting to emit machine data")
    
    #Store previous value of 'auto_preheat' to detect changes
    previous_auto_preheat = MeticulousConfig[CONFIG_USER].get('auto_preheat', None)

    while True:

        elapsed_time = time.time() - _time
        if (elapsed_time > 2 and not Machine.infoReady):
            _time = time.time()
            Machine.action("info")

        await sio.emit("status", {**Machine.data_sensors.to_sio()})

        if Machine.sensor_sensors is not None:
            water_status_dict = Machine.sensor_sensors.to_sio_water_status()
            water_status_value = water_status_dict["water_status"]
            await sio.emit("water_status", water_status_value)

        if sendInfoToFront:
            if Machine.sensor_sensors is not None:
                await sio.emit("sensors", Machine.sensor_sensors.to_sio_temperatures())
                await sio.emit("comunication", Machine.sensor_sensors.to_sio_communication())
                await sio.emit("actuators", Machine.sensor_sensors.to_sio_actuators())
            if Machine.esp_info is not None:
                await sio.emit("info", {**software_info, **Machine.esp_info.to_sio()})

        current_auto_preheat = MeticulousConfig[CONFIG_USER].get('auto_preheat')
        if current_auto_preheat != previous_auto_preheat:
            Machine.write(str.encode("action,auto_preheat,"+str(current_auto_preheat)+"\x03"))
            previous_auto_preheat = current_auto_preheat

        await sio.sleep(SAMPLE_TIME)
        i = i + 1

def send_data_loop():
    loop = asyncio.new_event_loop()
    asyncio.set_event_loop(loop)

    loop.run_until_complete(send_data())
    loop.close()

async def send_data():

    while (True):
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

        elif _input== "json":
            with open('fika.json','r') as openfile:
                json_file = json.load(openfile)
                # json_data = json.dumps(json_file, indent=1, sort_keys=False)
                # logger.info(json_data)
                Machine.send_json_with_hash(json_file)
                json_data=""
                json_file=""
            

        elif _input=="tare" or _input=="stop" or _input=="purge" or _input=="home" or _input=="start" :
            _input = "action,"+_input+"\x03"
            Machine.write(str.encode(_input))
            
        elif _input == "test":
            previous_sensor_status = MeticulousConfig[CONFIG_LOGGING][LOGGING_SENSOR_MESSAGES]
            MeticulousConfig[CONFIG_LOGGING][LOGGING_SENSOR_MESSAGES] = True
            for i in range(0,10):
                _input = "action,"+"purge"+"\x03"
                Machine.write(str.encode(_input))
                await asyncio.sleep(15)
                logger.info(_input)
                _input = "action,"+"home"+"\x03"
                Machine.write(str.encode(_input))
                await asyncio.sleep(15)
                contador = "Numero de prueba: "+str(i+1)
                logger.info(_input)
                logger.info(contador)
            MeticulousConfig[CONFIG_LOGGING][LOGGING_SENSOR_MESSAGES] = previous_sensor_status

        elif _input[:11] == "calibration":
             _input = "action,"+_input+"\x03"
             Machine.write(str.encode(_input))

        elif _input.startswith("update"):
            Machine.startUpdate()

        elif _input.startswith("notification"):
            notification = _input[12:]
            noti = Notification(notification, ["Ok", "Not okay"])
            noti.add_qrcode("Hello asjkdljlasjjkdsajkldasljkasdljk")
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
            await sio.emit("button", ButtonEventData.from_args(["encoder_button_pressed"]).to_sio())
        elif _input == "re":
            await sio.emit("button", ButtonEventData.from_args(["encoder_button_released"]).to_sio())

def main():
    global data_thread
    global send_data_thread
    parse_command_line()

    gatherVersionInfo()

    Machine.init(sio)

    send_data_thread = threading.Thread(target=send_data_loop)
    send_data_thread.start()

    GATTServer.getServer().start()

    WifiManager.init()
    NotificationManager.init(sio)
    ProfileManager.init()
    SoundPlayer.init(emulation=Machine.emulated)
    MeticulousConfig.setSIO(sio)

    handlers = [
        (r"/socket.io/", socketio.get_tornado_handler(sio)),
    ]

    if Machine.emulated:
        register_emulation_handlers()

    handlers.extend(API.get_routes())

    handlers.extend(WEB_UI_HANDLER)

    app = tornado.web.Application(
        handlers,
        debug=DEBUG,
    )

    app.listen(PORT)
    
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

if __name__ == "__main__":
    try:
        menu()
        main()
    except Exception as e:
        logger.exception("main() failed", exc_info=e, stack_info=True)
        exit(1)