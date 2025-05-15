import subprocess
import os
import re
from pathlib import Path
from datetime import datetime
from log import MeticulousLogger
from config import (
    MeticulousConfig,
    CONFIG_USER,
    UPDATE_CHANNEL,
    CONFIG_SYSTEM,
    LAST_SYSTEM_VERSIONS,
)
from notifications import Notification, NotificationManager, NotificationResponse

logger = MeticulousLogger.getLogger(__name__)

HAWKBIT_CONFIG_DIR = "/etc/hawkbit/"
HAWKBIT_CHANNEL_FILE = "channel"

BUILD_DATE_FILE = "/opt/ROOTFS_BUILD_DATE"
REPO_INFO_FILE = "/opt/summary.txt"
BUILD_CHANNEL_FILE = "/opt/image-build-channel"


class UpdateManager:

    ROOTFS_BUILD_DATE = None
    CHANNEL = None
    REPO_INFO = None

    is_changed = False

    @staticmethod
    def init():
        UpdateManager.setChannel(MeticulousConfig[CONFIG_USER][UPDATE_CHANNEL])

        build_time = UpdateManager.getBuildTimestamp()
        if build_time is None:
            logger.error("Could not get build timestamp")
            return

        build_channel = UpdateManager.getImageChannel()
        if build_channel is None:
            logger.error("Could not get build channel")
            return
        this_build_time = build_time.strftime("%Y%m%d_%H%M%S")
        this_version_string = build_channel + "-" + this_build_time
        try:
            # We might not have anything in the list, so we accept the exception
            last_known_version = MeticulousConfig[CONFIG_SYSTEM][LAST_SYSTEM_VERSIONS][
                -1
            ]
            is_changed = last_known_version != this_version_string
        except IndexError:
            is_changed = 1
            last_known_version = "NONE"

        print(f"Last known version image: {last_known_version}")
        print(f"This image version: {this_version_string}")

        if is_changed:
            logger.info(
                f"System was updated to {this_version_string} from {last_known_version}"
            )
            MeticulousConfig[CONFIG_SYSTEM][LAST_SYSTEM_VERSIONS].append(
                this_version_string
            )
            while len(MeticulousConfig[CONFIG_SYSTEM][LAST_SYSTEM_VERSIONS]) > 30:
                MeticulousConfig[CONFIG_SYSTEM][LAST_SYSTEM_VERSIONS].pop(0)
            MeticulousConfig.save()

            NotificationManager.add_notification(
                Notification(
                    message=f"System updated to [{build_channel}] build '{this_build_time}'. The UI will now be restarted to optimize the graphical performance.",
                    responses=[NotificationResponse.OK],
                    callback=lambda response: subprocess.run(
                        ["systemctl", "restart", "meticulous-dial"]
                    ),
                )
            )

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

        if not os.path.exists(BUILD_DATE_FILE):
            logger.warning(f"{BUILD_DATE_FILE} file not found")
            return None

        with open(BUILD_DATE_FILE, "r") as file:
            date_string = file.read().strip()

        try:
            build_time = datetime.strptime(date_string, "%a, %d %b %Y %H:%M:%S %z")
            return build_time
        except ValueError:
            logger.error("Invalid date format in BUILD_DATE file")
            return None

    @staticmethod
    def getImageChannel():
        if UpdateManager.CHANNEL is not None:
            return UpdateManager.CHANNEL
        try:
            with open(BUILD_CHANNEL_FILE, "r") as file:
                UpdateManager.CHANNEL = file.read().strip()
                logger.info(f"Read image build channel: {UpdateManager.CHANNEL}")
        except FileNotFoundError:
            logger.warning(f"{BUILD_CHANNEL_FILE} file not found")

        except Exception as e:
            logger.error(f"Error reading image build channel: {e}")

        return UpdateManager.CHANNEL

    @staticmethod
    def parse_summary_file(summary: str):

        data = {}
        current_repo_key = None
        modified_files_section = False

        # Regex to match lines like: ## ${COMPONENT} ##
        repo_header_pattern = re.compile(r"^##\s+(.*?)\s+##")

        lines = summary.splitlines()
        for i, line in enumerate(lines):
            line = line.strip()

            # If the line matches something like "## watcher ##"
            header_match = repo_header_pattern.match(line)
            if header_match:
                current_repo_key = header_match.group(1)
                data[current_repo_key] = {
                    "repo": None,
                    "url": None,
                    "branch": None,
                    "commit": None,
                    "last_commit": None,
                    "modified_files": [],
                }
                modified_files_section = False
                continue

            # If we haven't identified a repository block yet, skip
            if current_repo_key is None:
                continue

            # Check for lines like "Repository: xyz", "URL: xyz", etc.
            if line.startswith("Repository:"):
                data[current_repo_key]["repo"] = line.replace("Repository:", "").strip()
                modified_files_section = False
            elif line.startswith("URL:"):
                data[current_repo_key]["url"] = line.replace("URL:", "").strip()
                modified_files_section = False
            elif line.startswith("Branch:"):
                data[current_repo_key]["branch"] = line.replace("Branch:", "").strip()
                modified_files_section = False
            elif line.startswith("Commit:"):
                data[current_repo_key]["commit"] = line.replace("Commit:", "").strip()
                modified_files_section = False
            elif line.startswith("Last commit details:"):
                if len(lines) > i + 1:
                    data[current_repo_key]["last_commit"] = lines[i + 1].strip()
                modified_files_section = False
            elif line.startswith("Modified files:"):
                # After this line, all subsequent non-empty lines belong to the Modified files list,
                # until the next repo block or blank line.
                modified_files_section = True
            else:
                # If we're in the modified files section, any non-empty line is a filename
                if modified_files_section and line:
                    data[current_repo_key]["modified_files"].append(line)

        return data

    @staticmethod
    def getRepositoryInfo():
        if UpdateManager.REPO_INFO is not None:
            return UpdateManager.REPO_INFO

        try:
            with open(REPO_INFO_FILE, "r") as file:
                summary_file = file.read().strip()
                UpdateManager.REPO_INFO = UpdateManager.parse_summary_file(summary_file)
                logger.info(f"Read repository info: {UpdateManager.REPO_INFO}")
        except FileNotFoundError:
            logger.warning(f"{REPO_INFO_FILE} file not found")
        except Exception as e:
            logger.error(f"Error reading repository info: {e}")

        return UpdateManager.REPO_INFO

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
