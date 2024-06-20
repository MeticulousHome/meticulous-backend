import json
import os
import threading
import time
from datetime import datetime

import zstandard as zstd
from esp_serial.connection.emulation_data import EmulationData
from esp_serial.data import SensorData, ShotData
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

HISTORY_PATH = os.getenv("HISTORY_PATH", "/meticulous-user/history")


class Shot:
    def __init__(self) -> None:
        self.shotData = []
        self.profile = None
        self.profile_name = None
        self.startTime = time.time()

    def addSensorData(self, sensorData: SensorData):
        if len(self.shotData) > 0:
            # Append onto the last shotData
            self.shotData[-1]["sensors"] = dict(sensorData.__dict__)

    def addShotData(self, shotData: ShotData):
        from machine import Machine
        from profile import ProfileManager

        if self.profile_name is None and shotData.profile is not None:

            # Special case the emulation case
            if (
                Machine.emulated
                and shotData.profile == EmulationData.PROFILE_PLACEHOLDER
                and ProfileManager.get_last_profile() is not None
            ):
                self.profile_name = ProfileManager.get_last_profile()["profile"]["name"]
            else:
                self.profile_name = shotData.profile

            if self.profile is None:

                last_profile = ProfileManager.get_last_profile()

                if (
                    last_profile is not None
                    and last_profile.get("profile", None) is not None
                    and last_profile["profile"]["name"] == self.profile_name
                ):
                    self.profile = last_profile["profile"]
                else:
                    self.profile = {}

        # Shotdata is not json serialziable and we dont need the profile entry multiple times
        formated_data = {
            "shot": {
                "pressure": shotData.pressure,
                "flow": shotData.flow,
                "weight": shotData.weight,
                "temperature": shotData.temperature,
            },
            "time": shotData.time,
            "status": shotData.status,
        }
        self.shotData.append(formated_data)

    def to_json(self):
        shot_dict = {
            "time": datetime.fromtimestamp(self.startTime).strftime(
                "%Y_%m_%d_%H_%M_%S"
            ),
            "profile_name": self.profile_name,
            "data": self.shotData,
        }
        # empty dictionary evaluate to false
        if bool(self.profile):
            shot_dict["profile"] = self.profile
        return shot_dict


class ShotManager:
    _current_shot: Shot = None

    @staticmethod
    def start():
        ShotManager._current_shot = Shot()

    @staticmethod
    def handleSensorData(sensoData: SensorData):
        if sensoData is not None and ShotManager._current_shot is not None:
            ShotManager._current_shot.addSensorData(sensoData)

    @staticmethod
    def handleShotData(shotData: ShotData):
        if shotData is not None and ShotManager._current_shot is not None:
            ShotManager._current_shot.addShotData(shotData)

    @staticmethod
    def stop():
        if ShotManager._current_shot is not None:
            # Determine the folder path based on the shot start
            start = datetime.fromtimestamp(ShotManager._current_shot.startTime)

            folder_name = start.now().strftime("%Y-%m-%d")
            folder_path = os.path.join(HISTORY_PATH, folder_name)

            # Create the folder if it does not exist
            os.makedirs(folder_path, exist_ok=True)

            # Prepare the file path
            formatted_time = start.strftime("%H:%M:%S")
            file_name = f"{formatted_time}.shot.json.zst"
            file_path = os.path.join(folder_path, file_name)

            json_data = json.dumps(
                ShotManager._current_shot.to_json(), ensure_ascii=False
            )

            def compress_current_shot(data_json):
                # Compress and write the shot to disk
                logger.info("Writing and compressing shot file")
                start = time.time()
                with open(file_path, "wb") as file:
                    cctx = zstd.ZstdCompressor(level=22)
                    with cctx.stream_writer(file) as compressor:
                        compressor.write(data_json.encode("utf-8"))
                time_ms = (time.time() - start) * 1000
                logger.info(f"Writing json to disc took {time_ms} ms")

            compresson_thread = threading.Thread(
                target=compress_current_shot, args=(json_data,)
            )
            compresson_thread.start()

            # Clear tracking data after saving
            ShotManager._current_shot = None
