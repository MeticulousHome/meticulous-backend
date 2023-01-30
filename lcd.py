import serial
import time
import RPi.GPIO as GPIO
from dotenv import load_dotenv

load_dotenv()

lcd_en = 25
GPIO.setmode(GPIO.BCM)
GPIO.setup(lcd_en, GPIO.OUT)

try:
    while True:
        if os.environ.get("PCB_VERSION") == "V3.4":
            GPIO.output(lcd_en, 0)
        else:
            GPIO.output(lcd_en, 1)
except KeyboardInterrupt:
    GPIO.cleanup()