import subprocess
import re

from machine import Machine
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


class SoundVolumeController:
    @staticmethod
    def get_volume():
        if Machine.emulated:
            return 70

        try:
            result = subprocess.run(["pactl", "--", "get-sink-volume", "0"], capture_output=True, text=True, check=True)
            match = re.search(r"(\d+)%", result.stdout)
            if match:
                return int(match.group(1))
            logger.warning("Could not parse volume from pactl output")
            return None
        except Exception as e:
            logger.error(f"Failed to get volume: {e}")
            return None

    @staticmethod
    def set_volume(volume_percent):
        volume_percent = max(0, min(100, volume_percent))

        if Machine.emulated:
            logger.info(f"Emulated: Setting volume to {volume_percent}%")
            return

        try:
            subprocess.run(["pactl", "--", "set-sink-volume", "0", f"{volume_percent}%"], check=True)
        except Exception as e:
            logger.error(f"Failed to set volume: {e}")
