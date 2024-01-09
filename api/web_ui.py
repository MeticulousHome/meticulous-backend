import tornado.ioloop
import tornado.web
import tornado.escape
import json
import subprocess
from datetime import datetime
import time
import os

class SendDataHandler(tornado.web.RequestHandler):
    def post(self):
        #serial_manager.data_to_send = tornado.escape.json_decode(self.request.body)
        #print(f"Datos recibidos del cliente: {serial_manager.data_to_send}")
        self.write(json.dumps({"status": "success"}))

class SpeakerCommandHandler(tornado.web.RequestHandler):
    def post(self):
        #handle_speaker_command()
        self.write(json.dumps({"status": "success"}))

class ProgramESP32Handler(tornado.web.RequestHandler):
    def post(self):
        # Similar to your Flask code
        # ...
        pass

class SetTimeHandler(tornado.web.RequestHandler):
    def post(self):
        data = tornado.escape.json_decode(self.request.body)
        self.set_rtc_var_som(**data)
        self.write(json.dumps({"status": "success"}))

    def set_rtc_var_som(self, day, month, year, hour, minute, seconds):
        day = int(day)
        month = int(month)
        year = int(year)
        hour = int(hour)
        minute = int(minute)
        seconds = int(seconds)
        date_str = f"{year}{month:02d}{day:02d} {hour:02d}:{minute:02d}:{seconds:02d}"
        
        # Set system date and time
        subprocess.run(["date", "--set", date_str], check=True)
        time.sleep(0.1)

        # Synchronize the hardware clock with the system time
        subprocess.run(["hwclock", "--systohc", "--utc"], check=True)


class GetTimeHandler(tornado.web.RequestHandler):
    def get(self):
        current_time = self.get_current_time().strftime("%d/%m/%Y %H:%M:%S")
        self.write(json.dumps({"current_time": current_time}))
        
    def get_current_time(self):
        result = subprocess.run(["date"], capture_output=True, text=True)
        current_time_str = result.stdout.strip()
        # parse the datetime from the string
        current_time = datetime.strptime(current_time_str, "%a %d %b %Y %I:%M:%S %p %Z")
        return current_time


class GPIOStatusHandler(tornado.web.RequestHandler):
    def get(self):
        #gpio_value = handle_gpio_status()
        self.write(json.dumps({"gpiochip4_26": 0}))

WEB_UI_HANDLER = [
        (r'', tornado.web.RedirectHandler, {"url":"/"}),
        (r'/graph', tornado.web.RedirectHandler, {"url":"/debug/graph.html"}),
        (r'/debug', tornado.web.RedirectHandler, {"url":"/debug/graph.html"}),
        (r'/debug/(.*)', tornado.web.StaticFileHandler, {"default_filename": "graph.html","path": os.path.dirname(__file__)+"/web_ui"}),
        (r'/(.*)', tornado.web.StaticFileHandler, {"default_filename": "index.html","path": "/opt/meticulous-dashboard/"}),
    ]
