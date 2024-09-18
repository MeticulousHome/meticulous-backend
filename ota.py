import subprocess
from pathlib import Path

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

HAWKBIT_CONFIG_DIR = "/etc/hawkbit/"
HAWKBIT_CHANNEL_FILE = "channel"


class UpdateManager:

    @staticmethod
    def setChannel(channel: str):
        hawkbit_dir = Path(HAWKBIT_CONFIG_DIR)
        if not Path(hawkbit_dir).exists():
            logger.info(f"{hawkbit_dir} does not exist, not changing update channel")
            return

        channel_file = hawkbit_dir.joinpath(HAWKBIT_CHANNEL_FILE)
        try:
            with open(channel_file, "r") as f:
                current_channel = f.read()
        except FileNotFoundError:
            current_channel = None

        if current_channel != channel:
            try:
                with open(channel_file, "w") as f:
                    f.write(channel)
                logger.info(
                    f"Changed update channel from {current_channel} to {channel}"
                )
                subprocess.run(["systemctl", "restart", "rauc-hawkbit-updater"])
            except Exception as e:
                logger.error(f"Failed to change update channel: {e}")
