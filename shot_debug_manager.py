import json
import os
from named_thread import NamedThread
import time
import asyncio
from datetime import datetime, timedelta
import shutil
import zipfile

import zstandard as zstd

from config import (
    CONFIG_USER,
    CONFIG_WIFI,
    DEBUG_SHOT_DATA_RETENTION,
    MACHINE_DEBUG_SENDING,
    MeticulousConfig,
)
from esp_serial.data import SensorData, ShotData, MachineStatus, MachineStatusToProfile
from telemetry_service import TelemetryService
from log import MeticulousLogger
from shot_manager import Shot
import copy


logger = MeticulousLogger.getLogger(__name__)

DEBUG_FOLDER_FORMAT = "%Y-%m-%d"
DEBUG_FILE_FORMAT = "%H:%M:%S"
DEBUG_HISTORY_PATH = os.getenv("DEBUG_HISTORY_PATH", "/meticulous-user/history/debug")


class DebugShot(Shot):
    def __init__(self) -> None:
        from machine import Machine

        super().__init__()
        self.config = copy.deepcopy(MeticulousConfig[CONFIG_USER])
        self.config[CONFIG_WIFI] = {}
        self.machine = {}
        if Machine.esp_info is not None:
            self.machine = Machine.esp_info.to_sio()
        self.shottype = "shot"

    def to_json(self):
        data = {
            "time": self.startTime,
            "type": self.shottype,
            "profile_name": self.profile_name,
            "machine": self.machine,
            "profile": self.profile,
            "config": self.config,
            "data": self.shotData,
        }
        return data

    def append_shot_data(self, formated_data):
        time_passed = int((time.time() - self.startTime) * 1000.0)
        formated_data["profile_ms"] = time_passed
        self.shotData.append(formated_data)

    def set_shot_type(self, type: str):
        self.shottype = type


class ShotDebugManager:
    _current_data: DebugShot = None

    @staticmethod
    def start():
        if ShotDebugManager._current_data is None:
            try:
                ShotDebugManager._current_data = DebugShot()
                logger.info("Starting debug shot")
            except Exception as e:
                logger.error(f"Failed to start debug shot: {e}")
                ShotDebugManager._current_data = None
                return

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
            if (
                status in [MachineStatus.PURGE, MachineStatus.HOME, MachineStatus.BOOT]
                and MachineStatusToProfile.get(status, "") == profile
            ):
                ShotDebugManager._current_data.set_shot_type(status)

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
            if not os.path.isdir(os.path.join(DEBUG_HISTORY_PATH, f)):
                continue
            p = datetime.strptime(f, DEBUG_FOLDER_FORMAT)
            if p < cutoff_date:
                shutil.rmtree(os.path.join(DEBUG_HISTORY_PATH, f))
                logger.info(f"Deleted all shots in {f}")

    @staticmethod
    def zipAllDebugShots():
        retention_days = MeticulousConfig[CONFIG_USER][DEBUG_SHOT_DATA_RETENTION]
        if retention_days < 0:
            logger.info(
                "Debug shot data retention is disabled, not deleting old files"
            )  #
            return

        logger.info("Zipping all debug files")
        start = time.time()

        # Delete all potentially existing zip files except the one we are creating
        for root, dirs, files in os.walk(DEBUG_HISTORY_PATH):
            for file in files:
                if file.endswith(".zip"):
                    logger.info(f"Removing {file}")
                    os.remove(os.path.join(DEBUG_HISTORY_PATH, file))

        zip_name = datetime.now().strftime(
            f"debug-{DEBUG_FOLDER_FORMAT}-{DEBUG_FILE_FORMAT}.zip"
        )

        # Create a zipfile containing all the files in the debug history path
        zip_filename = os.path.join(DEBUG_HISTORY_PATH, zip_name)
        with zipfile.ZipFile(zip_filename, "w", zipfile.ZIP_DEFLATED) as zipf:
            for root, dirs, files in os.walk(DEBUG_HISTORY_PATH):
                for file in files:
                    if file.endswith(".zip") and file != zip_name:
                        logger.info(f"Removing {file}")
                        os.remove(os.path.join(DEBUG_HISTORY_PATH, file))
                    if file.endswith(".zst"):
                        file_path = os.path.join(root, file)
                        arcname = os.path.relpath(file_path, DEBUG_HISTORY_PATH)
                        zipf.write(file_path, arcname)

        time_ms = (time.time() - start) * 1000
        os.rename(zip_filename, os.path.join(DEBUG_HISTORY_PATH, zip_name))
        logger.info(f"Zipping all debug files disc took {time_ms} ms")
        return zip_name

    @staticmethod
    def stop():
        if ShotDebugManager._current_data is None:
            return
        
        if ShotDebugManager._current_data.profile is None:
            ShotDebugManager._current_data.profile = {}
            
        # Determine the folder path based on the current date
        start_timestamp = ShotDebugManager._current_data.startTime
        start = datetime.fromtimestamp(start_timestamp)
        folder_name = start.strftime(DEBUG_FOLDER_FORMAT)
        folder_path = os.path.join(DEBUG_HISTORY_PATH, folder_name)

        # Create the folder if it does not exist
        os.makedirs(folder_path, exist_ok=True)

        # Prepare the file path
        formatted_time = start.strftime(DEBUG_FILE_FORMAT)
        file_type = ShotDebugManager._current_data.shottype
        file_name = f"{formatted_time}.{file_type}.json.zst"
        file_path = os.path.join(folder_path, file_name)

        debug_shot_data = ShotDebugManager._current_data.to_json()
        if (
            not bool(debug_shot_data.get("profile"))
            and debug_shot_data.get("type") == "shot"
        ):
            from profiles import ProfileManager

            last_profile = ProfileManager.get_last_profile()
            if last_profile is not None:
                loadTime = last_profile.get("load_time", 0)
                loadDateTime = datetime.fromtimestamp(loadTime)
                loadTimeDiff = abs((start - loadDateTime).total_seconds())
                if loadTimeDiff:
                    logger.warning(
                        f"Profile load time ({loadDateTime}) and shot start time ({start}) are more than 30 seconds apart. Ignoring profile"
                    )
                else:
                    last_profile_name = last_profile.get("profile", {}).get("name")
                    logger.info(
                        f"Using last profile {last_profile_name} for debug shot"
                    )
                    debug_shot_data["profile"] = last_profile.get("profile")
                    if last_profile_name is not None and last_profile_name != "":
                        debug_shot_data["profile_name"] = last_profile_name

        data_json = json.dumps(debug_shot_data, ensure_ascii=False)

        async def compress_current_data(data_json):
            from machine import Machine

            # Compress and write the shot to disk
            logger.info("Writing and compressing debug file")
            start = time.time()

            # Compress to a byte array first
            cctx = zstd.ZstdCompressor(level=8)
            compressed_data = cctx.compress(data_json.encode("utf-8"))

            logger.info(f"Writing debug json to {file_path}")
            with open(file_path, "wb") as file:
                file.write(compressed_data)
            time_ms = (time.time() - start) * 1000
            logger.info(f"Writing debug json to disc took {time_ms} ms")

            if MeticulousConfig[CONFIG_USER][MACHINE_DEBUG_SENDING] is True:
                if Machine.emulated:
                    logger.info("Not sending emulated debug shots")
                else:
                    try:
                        await TelemetryService.upload_debug_shot(
                            compressed_data, file_name
                        )
                        logger.info("Debug shot data compressed and saved")

                    except Exception as e:
                        logger.error(f"Failed to send debug shot to server: {e}")

            compressed_data = None
            cctx = None
            data_json = None
            logger.info("Debug shot data compressed and saved")

            ShotDebugManager.deleteOldDebugShotData()

        def compression_loop(data_json):
            loop = asyncio.new_event_loop()
            asyncio.set_event_loop(loop)

            try:
                loop.run_until_complete(compress_current_data(data_json))
            finally:
                loop.close()

        compresson_thread = NamedThread(
            "DebugShotCompr", target=compression_loop, args=(data_json,)
        )
        compresson_thread.start()

        # Clear tracking data after saving
        ShotDebugManager._current_data = None
