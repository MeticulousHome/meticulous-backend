# Backend for rasp in esp

This repo is to run the backend of meticulous. 

For the correct functioning, you must create a ".env" file type and copy the following

    # PCB_VERSION = V3 # PCB with 2 ADS
    PCB_VERSION = V3.1 # PCB with 3 ADS

The sign **#** is for comments. The "V3" value refers to the FIKA PCB with two ADS and the V3.1 value refers to the FIKA PCB withs three ADS. 

# Read Serial

The *read_serial.py* file is to read de data from the ESP. The file has 3 functions and one class. 

## Read Line class

This class is to optimize the serial communication with a serial device. The class use a **bytearray** object and a **readline function**.

The bytearray object is used to create an array of bytes. 
The readline function reads a line and stores the information in the bytearray object. 

## read_arduino function

This function is used to read a serial device. The first steps are to enable the ESP, reset the serial device buffer and create a ReadLine object with the serial device as argument. 

The second step is reset the esp32. 

The third step is to start the collecting data in an infinite loop.

## send_data function 

This function is used to send data to the serial device. It is designed to interact with the test software. 

There is a thread to reset the rasp if the user enters the word "reset"

## reset_rasp function

This function is used to reset the rasp

## main function

There are two threads running in the main function. One is for sending data (executing the send data function) and the other is to read the serial device (executing the read arduino function).

# Backend

The *back.py* file is to run the backend. 

## Read Line class

This class is to optimize the serial communication with a serial device. The class use a **bytearray** object and a **readline function**.

The bytearray object is used to create an array of bytes. 
The readline function reads a line and stores the information in the bytearray object. 

## Set pins mode

From line 59 to line 66 is to setup the pins mode. There are 7 Pins:

1. Enable esp pin = 8
2. Enable lcd pin = 25
3. Enable button esp pin = 24
4. IO0 pin = 23
5. On off button pin = 18
6. Lcd flt pin = 7
7. Esp flt pin = 12

## Turn on/off functions

Turn on function is used to enable the esp and the lcd switches 
Turn off function is used to disable the lcd and the esp switches

## Kill the ui demo 

From line 76 to line 78 is to kill the LCD UI process. But this only works if the backend is not running automatically. 

## Libraries initialization

**keyboard = Controller()** is used to initialize a *Controller* object from the **pynput.keyboard library** which can be used to simulate keyboard input.

We define some command line options. The first is **"port"** and has a default value of 8080. It specifies the port number that the server should run on. 

The second is **"debug"** and has a default value of FALSE. It specifies whether the server should run in debug mode

Finally, the code initializes a **Socket.IO server** using the **AsyncServer class** from the **socketio library**. The server is configured to **allow cross-origin** resource sharing (CORS) from any origin (*), and to use the **tornado library's asynchronous mode**.
## Data sensors 

From line 88 to 95 is used to create a dictionary and save the data from the serial device. This data are:
1. Pressure
2. Flow
3. Weight
4. Temperature
5. Status
6. Time

## read_flt_pins function 

This function is used to read the pins flt from the switches. 

## enable_pcb function

This function is used to turn on the PCB by activating the switches. 

## read_on_off_bt()

This function is used to read the state of the on / off button. 

## Encoder functions

There are three functions for controlling the encoder and interacting with the LCD screen.

**cw_function** is used to identify when the user turns the encoder to the right
**ccw_function** is used to identify when the user turns the encoder to the left
**single_push** is used to identify when the user push the encoder

## reboot_esp function

This function is used to reboot the esp. 

## Connecting functions

There are functions to detect an event with the **@sio.event** decorator. The events are:
1. **connect:** This function detects when an application is connected. 
2. **disconnect:** This function detects when an application is disconnected.

## Event Handler functions

There are functions to detect an event with the **@sio.on** decorator. The events are:

1. **action:** This function is used to detect the "action" event and print its data to the console. The actions are *purge, home, start, stop, tare*
2. **parameters** This function is used to detect the "parameters" event. This event works by sending the parameters from the ipad aplication to the ESP in JSON format. 
3. **preset** This function is used to detect the "preset" event. This event is used to select the type of coffee that the meticulous should prepare and send the parameters to the ESP in JSON format. We have 6 types of coffe:
    1. breville
    2. cube
    3. diletta
    4. flair
    5. la-pavoni
    6. rocket

## read_arduino function

This function is used to read a serial device. The first step are reset the serial device buffer and create a ReadLine object with the serial device as argument. 

The second step is to start the collecting data in an infinite loop using the readline method from **ReadLine class**. 

The ESP (serial device) send data in specific format. This format is provided in the previous section called "data sensors".The code first splits the incoming data on the ',' character, resulting in a list of strings stored in data_str_sensors. We start collecting data if the first element of this list is the string "Data"

The code then compares the current value of the "status" key with the previous value stored in **old_status**, and if they are "heating" and "preinfusion" respectively, it sets the **time_flag** to **True** and records the current time using **time.time()**. If the value of the "status" key is "idle", the **time_flag** is set to **False**

If the **time_flag** is **True**, the code updates the value of the "time" key in the **data_sensors** dictionary to be the elapsed time since the **time_flag** was set to **True**. Otherwise, it sets the value of the **"time" key to 0**.

If the **incoming data** does not start with "Data", the code checks if it contains the strings "CCW", "CW", or "push", and calls the corresponding functions. If none of these strings are found, the code checks the value of the print_status and sensor_status variables and the first character of the incoming data. If certain conditions are met, it prints the incoming data to the console.

## async def live function

This function update the status and sensors values ot the ESP to the client over the Socket.IO server

## send_data function

This function sends actions to the esp. This actions are:
1. **reset**: Reset the esp
2. **show**: Show the data from the esp
3. **hide**: Hide the data from the esp
4. **json**: Send a JSON file from the raspberry pi to the esp
5. **tare**: 
6. **stop**: Stop the running action
7. **purge**: Purge the coffee machine
8. **home**: 