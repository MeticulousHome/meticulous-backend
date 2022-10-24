import time
import json
import serial
from pprint import pprint
import random

if __name__ == "__main__":
    print ("Ready...")
    ser  = serial.Serial("COM3", baudrate= 115200, 
           timeout=2.5, 
           parity=serial.PARITY_NONE, 
           bytesize=serial.EIGHTBITS, 
           stopbits=serial.STOPBITS_ONE
        )
    f = open('fika.json','r')
    data = json.load(f)
    # data["operation"] = "sequence"
    # data["operation"] = "Italyan"

    data=json.dumps(data)
    # print (data)
    if ser.isOpen():
        ser.write(data.encode('ascii'))
        ser.flush()
        try:
            incoming = ser.readline().decode("ascii")
            print (incoming)
        except Exception as e:
            print (e)
            pass
        ser.close()
    else:
        print ("opening error")