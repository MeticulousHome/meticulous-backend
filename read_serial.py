import serial
import time
import RPi.GPIO as GPIO
import threading 
from dotenv import load_dotenv
import os

load_dotenv()

#Class to optimize serial communication
    
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
                
arduino = serial.Serial('/dev/ttyS0',115200)

#class to read the Serial Communication
def read_arduino():
    
    start_time = time.time()
    arduino.reset_input_buffer()
    uart = ReadLine(arduino)

    old_status = ""
    time_flag = False 
    while True:
        data = uart.readline()
        if len(data) > 0:
            try:
                data_str = data.decode('utf-8')
            except:
                print("decoding fails")
                continue
            print(data_str)
       
#Class to select options menus       
def send_data():
    while (True):
        _input = input()
        if _input == "reset":
            tr = threading.Thread(target=reset_rasp)
            tr.deamon = True
            tr.start()
        else:
            arduino.write(str.encode(_input))
            
#Class to reset the rasp      
def reset_rasp():
    if os.environ.get("PCB_VERSION") == "V3":
        en=27
        io0=17

    elif os.environ.get("PCB_VERSION") == "V3.1":
        en=24
        io0=23
    else:
        en = 24
        io0 = 23
        print("Set pines to V3.1") 
    GPIO.setmode(GPIO.BCM)
    GPIO.setup(en, GPIO.OUT)
    GPIO.setup(io0, GPIO.OUT)
    GPIO.output(en, 0)
    GPIO.output(io0, 0)
    time.sleep(1)
    GPIO.output(io0, 1)
    time.sleep(1)
    GPIO.output(en, 1)
    time.sleep(1)
    GPIO.cleanup() 
    print("Raspberry is reseted")     
    
def main():
    t1 = threading.Thread(target=send_data)
    t1.deamon = True
    t1.start()
    t2 = threading.Thread(target=read_arduino)
    t2.deamon = True
    t2.start()
    
if __name__ == "__main__":
    try:
        main()
        
    except KeyboardInterrupt:
        GPIO.cleanup()
        print("An error was ocurred")
        