import RPi.GPIO as GPIO
import os
from dotenv import load_dotenv

load_dotenv()

GPIO.setmode(GPIO.BCM)
GPIO.setup(25, GPIO.OUT)
if os.environ.get("SWITCH_VERSION") == "V3.4":
    GPIO.output(25, 0)
    print("SWITCH_VERSION = V3.4")
else:
    GPIO.output(25, 1)

