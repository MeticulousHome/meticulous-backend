import subprocess
import os
from pathlib import Path
from datetime import datetime
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

HAWKBIT_CONFIG_DIR = "/etc/hawkbit/"
HAWKBIT_CHANNEL_FILE = "channel"


class UpdateManager:

    ROOTFS_BUILD_DATE = None
    BUILD_DATE_FILE = "/opt/ROOTFS_BUILD_DATE"

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

    @staticmethod
    def getBuildTimestamp():
        if UpdateManager.ROOTFS_BUILD_DATE is not None:
            return UpdateManager.ROOTFS_BUILD_DATE

        if not os.path.exists(UpdateManager.BUILD_DATE_FILE):
            logger.info("BUILD_DATE file not found")
            return None

        with open(UpdateManager.BUILD_DATE_FILE, "r") as file:
            date_string = file.read().strip()

        try:
            build_time = datetime.strptime(date_string, "%a, %d %b %Y %H:%M:%S %z")
            return build_time
        except ValueError:
            logger.error("Invalid date format in BUILD_DATE file")
            return None

    @staticmethod
    def forward_time():
        target_time = UpdateManager.getBuildTimestamp()
        if target_time is None:
            logger.error("Could not get build timestamp")
            return

        current_time = datetime.now(target_time.tzinfo)

        # Forward time only if it is older than the image itself
        if current_time < target_time:
            formatted_time = target_time.strftime("%Y-%m-%d %H:%M:%S")
            try:
                subprocess.run(["date", "-s", formatted_time], check=True)
                print(f"System time updated to: {formatted_time}")
            except subprocess.CalledProcessError as e:
                print(f"Failed to set system time: {e}")
