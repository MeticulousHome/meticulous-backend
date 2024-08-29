import json
import os
import threading
import time
import traceback
import uuid
from datetime import datetime
from pathlib import Path

import zstandard as zstd

from esp_serial.connection.emulation_data import EmulationData
from esp_serial.data import SensorData, ShotData
from log import MeticulousLogger
from shot_database import ShotDataBase, SearchParams, SearchOrder

logger = MeticulousLogger.getLogger(__name__)

HISTORY_PATH = os.getenv("HISTORY_PATH", "/meticulous-user/history")
SHOT_FOLDER = "shots"
SHOT_PATH = Path(HISTORY_PATH).joinpath(SHOT_FOLDER)


class Shot:
    def __init__(self) -> None:
        self.shotData = []
        self.profile = None
        self.profile_name = None
        self.startTime = time.time()
        self.id = str(uuid.uuid4())

    def addSensorData(self, sensorData: SensorData):
        if len(self.shotData) > 0:
            # Append onto the last shotData
            self.shotData[-1]["sensors"] = dict(sensorData.__dict__)

    def addShotData(self, shotData: ShotData):
        from profiles import ProfileManager

        from machine import Machine

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
            "time": self.startTime,
            "profile_name": self.profile_name,
            "data": self.shotData,
            "id": self.id,
        }
        # empty dictionary evaluate to false
        if bool(self.profile):
            shot_dict["profile"] = self.profile
        return shot_dict


class ShotManager:
    _last_shot: Shot = None
    _current_shot: Shot = None

    # The ShotDatabase is required to work once we use the ShotManager.
    # We therefore initialize it here
    @staticmethod
    def init():
        ShotDataBase.init()

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
    def _timestampToFilePaths(timestamp: float):
        start = datetime.fromtimestamp(timestamp)
        folder_name = Path(start.strftime("%Y-%m-%d"))

        formatted_time = start.strftime("%H:%M:%S")
        file_name = f"{formatted_time}.shot.json.zst"

        file_path = folder_name.joinpath(file_name)

        return (folder_name, file_path)

    @staticmethod
    def getCurrentShot():
        if not ShotManager._current_shot:
            return None

        formated_profile = {
            "db_key": None,
        }
        if ShotManager._current_shot.profile:
            profile = ShotManager._current_shot.profile
            formated_profile = {**formated_profile, **profile}

        (_folder_name, file_path) = ShotManager._timestampToFilePaths(
            ShotManager._current_shot.startTime
        )

        current_formated_shot = {
            "db_key": None,
            "file": str(file_path),
            **ShotManager._current_shot.to_json(),
            "profile": formated_profile,
        }

        return current_formated_shot

    @staticmethod
    def getLastShot():
        if not ShotManager._last_shot:
            results = ShotDataBase.search_history(
                SearchParams(sort=SearchOrder.descending, max_results=1)
            )
            if len(results) > 0:
                ShotManager._last_shot = results[0]
        return ShotManager._last_shot

    @staticmethod
    def stop():
        if ShotManager._current_shot is not None:

            shot_data = ShotManager._current_shot.to_json()
            if shot_data.get("profile") is None:
                from profiles import ProfileManager

                shot_data["profile"] = ProfileManager.get_last_profile()

            def write_current_shot(shot_data):
                # Determine the paths based on the shot start
                (folder_name, file_path) = ShotManager._timestampToFilePaths(
                    shot_data["time"]
                )

                # Compress and write the shot to disk
                logger.info("Writing and compressing shot file")
                start = time.time()

                try:
                    # Create the folder if it does not exist
                    os.makedirs(SHOT_PATH.joinpath(folder_name), exist_ok=True)
                    data_json = json.dumps(shot_data, ensure_ascii=False)
                    with open(SHOT_PATH.joinpath(file_path), "wb") as file:
                        cctx = zstd.ZstdCompressor(level=22)
                        with cctx.stream_writer(file) as compressor:
                            compressor.write(data_json.encode("utf-8"))

                except Exception as e:
                    logger.error(f"Failed to write shotfile to disk: {e}")
                    logger.error(traceback.format_exc())
                    return
                else:
                    time_ms = (time.time() - start) * 1000
                    logger.info(f"Writing json to disc took {time_ms} ms")

                # Add to SQLite database, it will compress automatically
                logger.info("Adding shot to sqlite database")
                start = time.time()

                try:
                    dbEntry = shot_data
                    dbEntry["file"] = str(file_path)
                    ShotDataBase.insert_history(shot_data)
                    ShotManager._last_shot = None
                    ShotManager.getLastShot()
                except Exception as e:
                    logger.error(f"Failed to insert shot into sqlite: {e}")
                    logger.error(traceback.format_exc())
                else:
                    time_ms = (time.time() - start) * 1000
                    logger.info(f"Ingesting shot into sqlite took {time_ms} ms")

            compresson_thread = threading.Thread(
                target=write_current_shot,
                args=(shot_data,),
            )
            compresson_thread.start()

            # Shift and clear shot handles after saving
            ShotManager._current_shot = None


def test():
    ShotManager.init()
    ShotManager.start()
    from profiles import ProfileManager

    dummyShotData = ShotData(
        pressure=0.0,
        flow=1.0,
        weight=2.0,
        temperature=3.0,
        status="closing valve",
        time=100,
        state="brewing",
        profile=ProfileManager.get_last_profile()["profile"]["name"],
        is_extracting=True,
    )
    ShotManager.handleShotData(dummyShotData)
    logger.info(json.dumps(ShotManager._current_shot.to_json()))
    ShotManager.stop()


if __name__ == "__main__":
    test()
