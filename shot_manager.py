import json
import os
import time
import zstandard as zstd
import threading
from datetime import datetime

from esp_serial.data import SensorData, ShotData
from datetime import datetime

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


class Shot:
    def __init__(self) -> None:
        self.shotData = []
        self.profile = None
        self.startTime = time.time()

    def addSensorData(self, sensorData: SensorData, time_ms):
        if len(self.shotData) > 0:
            self.shotData[-1]["sensors"] = dict(sensorData.__dict__)

    def addShotData(self, shotData: ShotData):
        if shotData.profile is not None:
            logger.info(shotData.profile)
        if self.profile is None and shotData.profile is not None:
            self.profile = shotData.profile
        # Shotdata is not json serialziable and we dont need the profile entry multiple times
        formated_data = {
            "shot": {
                "pressure": shotData.pressure,
                "flow": shotData.flow,
                "weight": shotData.weight,
                "temperature": shotData.temperature,
            },
            "time": shotData.time,
            "status": shotData.status
        }
        self.shotData.append(formated_data)

    def to_json(self):
        shot_dict = {
            "time":  datetime.fromtimestamp(self.startTime).strftime("%Y_%m_%d_%H_%M_%S"),
            "profile": self.profile,
            "data": self.shotData,
        }
        return shot_dict


class ShotManager:
    SHOT_FOLDER = "./shots"
    _current_shot: Shot = None

    @staticmethod
    def start():
        ShotManager._current_shot = Shot()

    @staticmethod
    def handleSensorData(sensoData: SensorData, time_ms):
        if sensoData is not None and ShotManager._current_shot is not None:
            ShotManager._current_shot.addSensorData(sensoData, time_ms)

    @staticmethod
    def handleShotData(shotData: ShotData):
        if shotData is not None and ShotManager._current_shot is not None:
            ShotManager._current_shot.addShotData(shotData)

    @staticmethod
    def stop():
        if ShotManager._current_shot is not None:
            # Determine the folder path based on the current date
            folder_name = datetime.now().strftime("%Y-%m-%d")
            folder_path = os.path.join(ShotManager.SHOT_FOLDER, folder_name)

            # Create the folder if it does not exist
            os.makedirs(folder_path, exist_ok=True)

            # Prepare the file path
            file_name = f"{time.time()}.shot.json.zst"
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
                logger.info(
                    f"Writing json to disc took {time_ms} ms"
                )

            compresson_thread = threading.Thread(target=compress_current_shot,args=(json_data,))
            compresson_thread.start()

            # Clear tracking data after saving
            ShotManager._current_shot = None
