import csv
import io
import os
from named_thread import NamedThread
import time
import asyncio
from datetime import datetime, timedelta
import shutil

import zstandard as zstd

from config import (
    CONFIG_USER,
    DEBUG_SHOT_DATA_RETENTION,
    MACHINE_DEBUG_SENDING,
    MeticulousConfig,
)
from esp_serial.data import SensorData, ShotData
from telemetry_service import TelemetryService
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

DEBUG_FOLDER_FORMAT = "%Y-%m-%d"
DEBUG_FILE_FORMAT = "%H:%M:%S"
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
    _current_data_type: str = "shot"

    @staticmethod
    def start():
        if ShotDebugManager._current_data is None:
            ShotDebugManager._current_data = DebugData()
            ShotDebugManager._current_data_type = "shot"
            logger.info("Starting debug shot")

    @staticmethod
    def handleSensorData(sensoData: SensorData):
        if sensoData is not None and ShotDebugManager._current_data is not None:
            ShotDebugManager._current_data.addSensorData(sensoData)

    @staticmethod
    def handleShotData(shotData: ShotData):
        if shotData is not None and ShotDebugManager._current_data is not None:
            ShotDebugManager._current_data.addShotData(shotData)
            status = shotData.status
            profile = shotData.profile
            if status in ["purge", "home"] and profile == status.capitalize():
                ShotDebugManager._current_data_type = status

    @staticmethod
    def deleteOldDebugShotData():
        retention_days = MeticulousConfig[CONFIG_USER][DEBUG_SHOT_DATA_RETENTION]
        if retention_days < 0:
            logger.info(
                "Debug shot data retention is disabled, not deleting old files"
            )  #
            return

        logger.info(
            f"Debug shot data retention is set to {retention_days} days, deleting old files"
        )
        cutoff_date = datetime.now() - timedelta(days=retention_days)
        history_folders = os.listdir(DEBUG_HISTORY_PATH)
        for f in history_folders:
            p = datetime.strptime(f, DEBUG_FOLDER_FORMAT)
            if p < cutoff_date:
                shutil.rmtree(os.path.join(DEBUG_HISTORY_PATH, f))
                logger.info(f"Deleted all shots in {f}")

    @staticmethod
    def stop():
        if ShotDebugManager._current_data is None:
            return

        # Determine the folder path based on the current date
        start_timestamp = ShotDebugManager._current_data.startTime
        start = datetime.fromtimestamp(start_timestamp)
        folder_name = start.strftime(DEBUG_FOLDER_FORMAT)
        folder_path = os.path.join(DEBUG_HISTORY_PATH, folder_name)

        # Create the folder if it does not exist
        os.makedirs(folder_path, exist_ok=True)

        # Prepare the file path
        formatted_time = start.strftime(DEBUG_FILE_FORMAT)
        file_type = ShotDebugManager._current_data_type
        file_name = f"{formatted_time}.{file_type}.csv.zst"
        file_path = os.path.join(folder_path, file_name)

        csv_data = ShotDebugManager._current_data.to_csv()

        async def compress_current_data(data_json):
            # Compress and write the shot to disk
            logger.info("Writing and compressing debug file")
            start = time.time()

            # Compress to a byte array first
            cctx = zstd.ZstdCompressor(level=8)
            compressed_data = cctx.compress(data_json.encode("utf-8"))

            logger.info(f"Writing debug csv to {file_path}")
            with open(file_path, "wb") as file:
                file.write(compressed_data)
            time_ms = (time.time() - start) * 1000
            logger.info(f"Writing debug csv to disc took {time_ms} ms")
            data_json = None

            if MeticulousConfig[CONFIG_USER][MACHINE_DEBUG_SENDING] is True:
                try:
                    await TelemetryService.upload_debug_shot(compressed_data)
                except Exception as e:
                    logger.error(f"Failed to send debug shot to server: {e}")

            compressed_data = None
            cctx = None
            data_json = None
            logger.info("Debug shot data compressed and sent")

            ShotDebugManager.deleteOldDebugShotData()

        def compression_loop(csv_data):
            loop = asyncio.new_event_loop()
            asyncio.set_event_loop(loop)

            try:
                loop.run_until_complete(compress_current_data(csv_data))
            finally:
                loop.close()

        compresson_thread = NamedThread(
            "DebugShotCompr", target=compression_loop, args=(csv_data,)
        )
        compresson_thread.start()

        # Clear tracking data after saving
        ShotDebugManager._current_data = None
