import RPi.GPIO as GPIO
import os
from dotenv import load_dotenv

load_dotenv()

GPIO.setmode(GPIO.BCM)
GPIO.setup(25, GPIO.OUT)
if os.environ.get("EN_PIN_HIGH") == "0":
    GPIO.output(25, 0)
    print("EN_PIN_HIGH = 0")
elif os.environ.get("EN_PIN_HIGH") == "1":
    GPIO.output(25, 1)
    print("EN_PIN_HIGH = 1")
else:
    GPIO.output(25, 0)
    print("EN_PIN_HIGH = 0 por default")

