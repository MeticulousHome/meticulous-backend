from tornado.options import define, options, parse_command_line
import socketio
import tornado.web
import tornado.ioloop
import traceback
import serial
import threading


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

def read_arduino():
    arduino = serial.Serial("COM3",115200)
    while True:
        
        data =arduino.readline().decode('utf-8')
        sensors_str = data.split(',')
        arduino.write(b'32\n')
        # if sensors_str[0] == 'Data':
            # print("Hahaha") <-- flag

            # print(pressure,flow,weight,temperature,status)
        return sensors_str
    
def data_treatment():
    while True:
        data =read_arduino()
        if data[0] == 'Data':
            data_sensors["pressure"] = data[1]
            data_sensors["flow"]= data[2]
            data_sensors["weight"] = data[3]
            data_sensors["temperature"] = data[4]
            status_bad = data[5]
            data_sensors["status"] = status_bad.strip("\n")
            return data_sensors

async def live():

    global coffee_status

    # RotaryEncoder(down_switch,up_switch,menu_switch, lambda event: asyncio.run(tuner_event(event)))

#     with open('./json/profile.json') as json_file:
#         json_object = json.load(json_file)
#         json_file.close()

    process_started = False
    SAMPLE_TIME = 1
    elapsed_time = 0
    i = 0
    while True:
        await sio.emit("status", {
            "name": "idle",
            "sensors": {
                "p": data_sensors["pressure"],
                "f": data_sensors["flow"],
                "w": data_sensors["weight"],
                "t": ["temperature"],
            },
            "time": 0
        })
        await sio.sleep(SAMPLE_TIME)
        i = i + 1

    
def main():
    parse_command_line()

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