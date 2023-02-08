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
                
arduino = serial.Serial('/dev/ttyS0',115200) #Arduino port declaration


esp_en = 8 #Enable ESP pin declaration
GPIO.setmode(GPIO.BCM)
GPIO.setup(esp_en , GPIO.OUT)

#function to read the Serial Communication
def read_arduino():
    if os.environ.get("SWITCH_VERSION") == "V3.4":
        GPIO.output(esp_en, 0) #Enable the ESP
        print("SWITCH_VERSION = V3.4") 
    else:
        GPIO.output(esp_en, 1) #Enable the ESP
    arduino.reset_input_buffer() #A Serial function member to reset the buffer
    uart = ReadLine(arduino) #Serial communication optimization class declaration  with a serial device as an argument
    
    #Reset the rasp before obtaining data from the serial device (in this case an ESP device)
    
    tr_1 = threading.Thread(target=reset_rasp)
    tr_1.deamon = True
    tr_1.start()
    

    
    #Start data collection
    
    while True:
        data = uart.readline() #read bits from the serial device
        if len(data) > 0:
            try:
                data_str = data.decode('utf-8') #decode the information
            except:
                print("decoding fails")
                continue
            print(data_str) #print the information
       
#function to select options menus          
#This function is only used to interact with the test software.    
def send_data():
    while (True):
        _input = input() #Function for entering an option
        if _input == "reset": #If the prompt is "reset", call the thread to reset the ESP
            tr = threading.Thread(target=reset_rasp)
            tr.deamon = True
            tr.start()
        else:
            arduino.write(str.encode(_input)) #else encode the option to interact with the test software
            
#function to reset the rasp      
def reset_rasp():
    #Pins choose according to the PCB version. 
    if os.environ.get("PINES_VERSION") == "V3":
        en=27
        io0=17
        print("Set pines to V3") 
    elif os.environ.get("PINES_VERSION") == "V3.1":
        en=24
        io0=23
        print("Set pines to V3.1") 
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
    t1 = threading.Thread(target=send_data) #Thread to send data
    t1.deamon = True
    t1.start()
    t2 = threading.Thread(target=read_arduino) #Thread to read the serial device
    t2.deamon = True
    t2.start()
    
if __name__ == "__main__":
    try:
        main()
        
    except KeyboardInterrupt:
        GPIO.cleanup()
        print("An error was ocurred")
        