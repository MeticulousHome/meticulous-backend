import json
import os
import time
import zstandard as zstd
import threading
import csv

from datetime import datetime

from esp_serial.data import SensorData, ShotData
from datetime import datetime

from config import *

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

DEBUG_HISTORY_PATH = os.getenv(
    "DEBUG_HISTORY_PATH", '/meticulous-user/history/debug')


class DebugData:
    def __init__(self) -> None:
        self.shotData = []
        self.startTime = time.time()

    def addSensorData(self, sensorData: SensorData):
        if len(self.shotData) > 0:
            # Append onto the last shotData
            self.shotData[-1].update(dict(sensorData.__dict__))

    def addShotData(self, shotData: ShotData):
        # Shotdata is not json serialziable and we dont need the profile entry multiple times
        formated_data = {
            "pressure": shotData.pressure,
            "flow": shotData.flow,
            "weight": shotData.weight,
            "temperature": shotData.temperature,
            "time": shotData.time,
            "profile_time": time.time() - self.startTime,
            "status": shotData.status,
            "profile": shotData.profile
        }
        self.shotData.append(formated_data)

    def to_csv(self, file_path=None):
        emptyShot = ShotData()
        emptySensors = SensorData()

        shotFields = list(emptyShot.__dict__.keys())
        sensorFields = list(emptySensors.__dict__.keys())
        classFields = ["startTime"]

        allFields = shotFields + sensorFields + classFields

        import io
        output = io.StringIO()
        writer = csv.DictWriter(output, fieldnames=allFields)
        writer.writeheader()

        for shotSample in self.shotData:
            row = {
                "startTime": self.startTime,
            }
            for sensor_field in shotFields + sensorFields:
                row[sensor_field] = shotSample.get(sensor_field, '')
            writer.writerow(row)
        result = output.getvalue()
        output.close()

        return result


class ShotDebugManager:
    _current_data: DebugData = None

    @staticmethod
    def start():
        if not MeticulousConfig[CONFIG_USER][DEBUG_SHOT_DATA]:
            return

        if ShotDebugManager._current_data is None:
            ShotDebugManager._current_data = DebugData()
            logger.info("Starting debug shot")

    @staticmethod
    def handleSensorData(sensoData: SensorData):
        if sensoData is not None and ShotDebugManager._current_data is not None:
            ShotDebugManager._current_data.addSensorData(sensoData)

    @staticmethod
    def handleShotData(shotData: ShotData):
        if shotData is not None and ShotDebugManager._current_data is not None:
            ShotDebugManager._current_data.addShotData(shotData)

    @staticmethod
    def stop():
        if ShotDebugManager._current_data is not None:
            # Determine the folder path based on the current date
            start = datetime.fromtimestamp(ShotDebugManager._current_data.startTime)
            folder_name = start.strftime("%Y-%m-%d")
            folder_path = os.path.join(DEBUG_HISTORY_PATH, folder_name)

            # Create the folder if it does not exist
            os.makedirs(folder_path, exist_ok=True)

            # Prepare the file path
            formatted_time = start.strftime('%H:%M:%S')
            file_name = f"{formatted_time}.debug.csv.zst"
            file_path = os.path.join(folder_path, file_name)

            csv_data = ShotDebugManager._current_data.to_csv()

            def compress_current_data(data_json):
                # Compress and write the shot to disk
                logger.info("Writing and compressing debug file")
                start = time.time()
                with open(file_path, "wb") as file:
                    cctx = zstd.ZstdCompressor(level=22)
                    with cctx.stream_writer(file) as compressor:
                        compressor.write(data_json.encode("utf-8"))
                time_ms = (time.time() - start) * 1000
                logger.info(
                    f"Writing debug csv to disc took {time_ms} ms"
                )

            compresson_thread = threading.Thread(
                target=compress_current_data, args=(csv_data,))
            compresson_thread.start()

            # Clear tracking data after saving
            ShotDebugManager._current_data = None
