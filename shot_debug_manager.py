import csv
import io
import os
from named_thread import NamedThread
import time
import sentry_sdk
from datetime import datetime

import zstandard as zstd

from config import CONFIG_USER, DEBUG_SHOT_DATA, MACHINE_DEBUG_SENDING, MeticulousConfig
from esp_serial.data import SensorData, ShotData
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

DEBUG_HISTORY_PATH = os.getenv("DEBUG_HISTORY_PATH", "/meticulous-user/history/debug")


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
            "gravimetric_flow": shotData.gravimetric_flow,
            "time": shotData.time,
            "profile_time": time.time() - self.startTime,
            "status": shotData.status,
            "profile": shotData.profile,
            "main_controller_kind": shotData.main_controller_kind,
            "main_setpoint": shotData.main_setpoint,
            "aux_controller_kind": shotData.aux_controller_kind,
            "aux_setpoint": shotData.aux_setpoint,
            "is_aux_controller_active": shotData.is_aux_controller_active,
        }
        self.shotData.append(formated_data)

    def to_csv(self, file_path=None):
        emptyShot = ShotData()
        emptySensors = SensorData()

        shotFields = list(emptyShot.__dict__.keys())
        sensorFields = list(emptySensors.__dict__.keys())
        classFields = ["startTime", "profile_ms"]

        allFields = shotFields + sensorFields + classFields

        output = io.StringIO()
        writer = csv.DictWriter(output, fieldnames=allFields)
        writer.writeheader()

        for shotSample in self.shotData:
            row = {
                "startTime": self.startTime,
                "profile_ms": round(shotSample["profile_time"] * 1000),
            }
            for sensor_field in shotFields + sensorFields:
                row[sensor_field] = shotSample.get(sensor_field, "")
            writer.writerow(row)
        result = output.getvalue()
        output.close()

        return result


class ShotDebugManager:
    _current_data: DebugData = None

    @staticmethod
    def start():
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
            start_timestamp = ShotDebugManager._current_data.startTime
            start = datetime.fromtimestamp(start_timestamp)
            folder_name = start.strftime("%Y-%m-%d")
            folder_path = os.path.join(DEBUG_HISTORY_PATH, folder_name)

            # Create the folder if it does not exist
            os.makedirs(folder_path, exist_ok=True)

            # Prepare the file path
            formatted_time = start.strftime("%H:%M:%S")
            file_name = f"{formatted_time}.debug.csv.zst"
            file_path = os.path.join(folder_path, file_name)

            csv_data = ShotDebugManager._current_data.to_csv()

            def compress_current_data(data_json):
                # Compress and write the shot to disk
                logger.info("Writing and compressing debug file")
                start = time.time()

                # Compress to a byte array first
                cctx = zstd.ZstdCompressor(level=8)
                compressed_data = cctx.compress(data_json.encode("utf-8"))

                # Only write to file if enabled
                if MeticulousConfig[CONFIG_USER][DEBUG_SHOT_DATA]:
                    logger.info(f"Writing debug csv to {file_path}")
                    with open(file_path, "wb") as file:
                        file.write(compressed_data)
                    time_ms = (time.time() - start) * 1000
                    logger.info(f"Writing debug csv to disc took {time_ms} ms")
                    data_json = None
                else:
                    logger.info("Debug shot data is disabled, skipping writing to disk")

                if MeticulousConfig[CONFIG_USER][MACHINE_DEBUG_SENDING] is True:
                    scope = sentry_sdk.new_scope()
                    scope.add_attachment(bytes=compressed_data, filename=file_path)
                    scope.set_context("config", MeticulousConfig.copy())
                    scope.capture_message("Debug shot data", level="info", scope=scope)

            compresson_thread = NamedThread(
                "DebugShotCompr", target=compress_current_data, args=(csv_data,)
            )
            compresson_thread.start()

            # Clear tracking data after saving
            ShotDebugManager._current_data = None
