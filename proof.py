import serial
import threading
# sensores_str = 0
data_sensors = {
    "pressure":1,
    "flow":2,
    "weight":3,
    "temperature":4,
    "status":5
}
def read_arduino():
    arduino = serial.Serial("COM3",115200)
    arduino.reset_input_buffer()
    arduino.write(b'32\n')
    while True:
        data = arduino.readline().decode('utf-8')
        if len(data) > 0:
            print(data)
        # sensors_str =arduino.readline().decode('ascii')
        # sensors_str =arduino.readline().decode('utf-8')
        # print(sensors_str)
        # data = sensors_str.split(',')
        # if data[0] == 'Data':
        #     data_sensors["pressure"] = data[1]
        #     data_sensors["flow"]= data[2]
        #     data_sensors["weight"] = data[3]
        #     data_sensors["temperature"] = data[4]
        #     status_bad = data[5]
        #     data_sensors["status"] = status_bad.strip("\n")
        #     print(data_sensors)
        # if sensors_str[0] == 'Data':
            # print("Hahaha") <-- flag

            # print(pressure,flow,weight,temperature,status)
        # return sensors_str  
        
        # if():
        
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
            print(data_sensors)
            # return data_sensors
            
            
if __name__ == "__main__":
    # data_thread = threading.Thread(target=data_treatment)
    # data_thread.daemon = True
    # data_thread.start()
    # print(data_sensors)
    read_arduino()