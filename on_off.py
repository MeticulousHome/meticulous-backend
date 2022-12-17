import serial
import time
import RPi.GPIO as GPIO

on_off_bt = 18
GPIO.setmode(GPIO.BCM)
GPIO.setup(on_off_bt, GPIO.IN)

try:
    while True:
        # GPIO.output(on_off_bt, 1)
        print(GPIO.input(on_off_bt))
except KeyboardInterrupt:
    GPIO.cleanup()