sd#Backend for rasp in esp

This repo is to run the backend of meticulous. 

For the correct functioning, you must create a ".env" file type and copy the following

    # PCB_VERSION = V3 # PCB with 2 ADS
    PCB_VERSION = V3.1 # PCB with 3 ADS

The sign **#** is for comments. The "V3" value refers to the FIKA PCB with two ADS and the V3.1 value refers to the FIKA PCB withs three ADS. 

## Read Serial

The *read_serial.py* file is to read de data from the ESP. The file has 3 functions and one class. 

### Read Line class

This class is to optimize the serial communication with a serial device. The class use a **bytearray** object and a **readline function**.

The bytearray object is used to create an array of bytes. 
The readline function reads a line and stores the information in the bytearray object. 

### read_arduino function

This function is used to read a serial device. The first steps are to enable the ESP, reset the serial device buffer and create a ReadLine object with the serial device as argument. 

The second step 



## Backend

The *back.py* file is to run the backend. 

