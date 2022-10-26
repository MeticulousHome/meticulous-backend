import json
import time
import serial


if __name__ == "__main__":
    print ("Ready...")
    ser  = serial.Serial("COM3", baudrate= 115200)
    f = open('fika.json','r')
    data = json.load(f)

    data = json.dumps(data,indent = 1)
    data = "*"+data+"#"
    ser.write(data.encode("utf-8"))
    init_while1 = True
    while (init_while1):
        if ser.inWaiting() > 0:
            data_from_esp32 = ser.read().decode('utf-8')
            print(data_from_esp32, end="")
            if  data_from_esp32 == "#":
                init_while1 = False

    # while (True):
    #     if ser.inWaiting() > 0:
    #         data_from_esp32 = ser.read().decode('utf-8')
    #         print(data_from_esp32, end="")