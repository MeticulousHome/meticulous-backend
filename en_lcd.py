import RPi.GPIO as GPIO
from dotenv import load_dotenv

GPIO.setmode(GPIO.BCM)
GPIO.setup(25, GPIO.OUT)
if os.environ.get("PCB_VERSION") == "V3.4":
    GPIO.output(lcd_en, 0)
else:
    GPIO.output(25, 1)

