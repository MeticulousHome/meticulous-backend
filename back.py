from tornado.options import define, options, parse_command_line
import socketio
import tornado.web
import tornado.ioloop
import traceback
import serial
import threading
import time
from pynput.keyboard import Key, Controller

keyboard = Controller()

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

def cw_function():
    keyboard.press(Key.right)
    keyboard.release(Key.right)
    print("RIGHT!")

def ccw_function():
    keyboard.press(Key.left)
    keyboard.release(Key.left)
    print("LEFT!")

def single_push():
    keyboard.press(Key.space)
    keyboard.release(Key.space)
    print("CLICK!")

@sio.event
def connect(sid, environ):
    print('connect ', sid)

@sio.event
def disconnect(sid):
    print('disconnect ', sid)

# arduino = serial.Serial("COM3",115200)
arduino = serial.Serial('/dev/ttyS0',115200)
start_time = time.time()

def read_arduino():
    global start_time

    # arduino = serial.Serial("COM3",115200)
    arduino.reset_input_buffer()
    arduino.write(b'32\n')
    uart = ReadLine(arduino)

    old_status = ""
    while True:
        data = uart.readline()
        if len(data) > 0:
            # data_bit = bytes(data)
            try:
                data_str = data.decode('utf-8')
            except:
                print("decoding fails")
                continue

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

                if (c1 and c2):
                    start_time = time.time()
                    print("start_time: {:.1f}".format(start_time))

                old_status = data_sensors["status"]
                # print(data_sensors["status"])
            elif data_str.find("CCW") > -1:
                ccw_function()
            elif data_str.find("CW") > -1:
                cw_function()
            elif data_str.find("push") > -1:
                single_push()
            else:
                # print(data_str_sensors[0])
                print(data_str)
    
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
            "time": str(time.time() - start_time)
        })
        await sio.sleep(SAMPLE_TIME)
        i = i + 1

    
def main():
    parse_command_line()
    data_thread = threading.Thread(target=data_treatment)
    # data_thread.daemon = True
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