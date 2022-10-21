import serial

def read_arduino():
    arduino = serial.Serial("COM3",115200)
    while True:
        
        data =arduino.readline().decode('ascii')
        sensores_str = data.split(',')
        print(sensores_str)
            
if __name__ == "__main__":
    read_arduino()