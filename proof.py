import serial
import threading
# sensores_str = 0

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
    
    uart = ReadLine(arduino)
    
    while True:
        data = uart.readline()
        if len(data) > 0:
            data_bit = bytes(data)
            data_str = data_bit.decode('utf-8')
            # print(data_str)
            # sensors_str =data.readline().decode('ascii')
            # # sensors_str =data.readline().decode('utf-8')
            # print(sensors_str)
            data_str_sensors = data_str.split(',')
            # print(data_str_sensors)
            if data_str_sensors[0] == 'Data':
                data_sensors["pressure"] = data_str_sensors[1]
                data_sensors["flow"]= data_str_sensors[2]
                data_sensors["weight"] = data_str_sensors[3]
                data_sensors["temperature"] = data_str_sensors[4]
                status_bad = data_str_sensors[5]
                data_sensors["status"] = status_bad.strip("\n")
                print(data_sensors)
        # if sensors_str[0] == 'Data':
            # print("Hahaha") <-- flag

            # print(pressure,flow,weight,temperature,status)
        # return sensors_str  
        
        # if():
        
def data_treatment():
    read_arduino()
            
            
if __name__ == "__main__":
    # data_thread = threading.Thread(target=read_arduino)
    # data_thread.daemon = True
    # data_thread.start()
    # # print(data_sensors)
    read_arduino()