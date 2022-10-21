from tornado.options import define, options, parse_command_line
import socketio
import tornado.web
import tornado.ioloop
import traceback
import serial
import threading
import time


define("port", default=8080, help="run on the given port", type=int)
define("debug", default=False, help="run in debug mode")

sio = socketio.AsyncServer(cors_allowed_origins='*', async_mode='tornado')
data_sensors = {
    "pressure":1,
    "flow":2,
    "weight":3,
    "temperature":4,
    "status":5
}

@sio.event
def connect(sid, environ):
    print('connect ', sid)

@sio.event
def disconnect(sid):
    print('disconnect ', sid)

arduino = serial.Serial("COM3",115200)

def read_arduino():
    while True:
        data =arduino.readline().decode('utf-8')
        sensors_str = data.split(',')
        if sensors_str[0] == 'Data':
            data_sensors["pressure"] = sensors_str[1]
            data_sensors["flow"]= sensors_str[2]
            data_sensors["weight"] = sensors_str[3]
            data_sensors["temperature"] = sensors_str[4]
            status_bad = sensors_str[5]
            data_sensors["status"] = status_bad.strip("\n")
    
def data_treatment():
    time.sleep(0.1)
    arduino.write(b'32\n')
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
    while True:
        await sio.emit("status", {
            "name": data_sensors["status"],
            "sensors": {
                "p": data_sensors["pressure"],
                "f": data_sensors["flow"],
                "w": data_sensors["weight"],
                "t": data_sensors["temperature"],
            },
            "time": 0
        })
        await sio.sleep(SAMPLE_TIME)
        i = i + 1

    
def main():
    parse_command_line()
    data_thread = threading.Thread(target=data_treatment)
    data_thread.daemon = True
    data_thread.start()
    app = tornado.web.Application(
        [
            (r"/socket.io/", socketio.get_tornado_handler(sio)),
        ],
        debug=options.debug,
    )

    app.listen(options.port)
    
    sio.start_background_task(live)
    tornado.ioloop.IOLoop.current().start()

if __name__ == "__main__":
    try:
        main()
        
    except:
        traceback.print_exc()