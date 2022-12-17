import serial
import time
import RPi.GPIO as GPIO

lcd_en = 25
GPIO.setmode(GPIO.BCM)
GPIO.setup(lcd_en, GPIO.OUT)

try:
    while True:
        GPIO.output(lcd_en, 1)
except KeyboardInterrupt:
    GPIO.cleanup()