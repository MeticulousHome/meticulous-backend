import json
import aiohttp
from sentry_sdk import capture_message

from config import (
    CONFIG_USER,
    TELEMETRY_SERVICE_ENABLED,
    MeticulousConfig,
)
from log import MeticulousLogger
from notifications import Notification, NotificationManager, NotificationResponse
from hostname import HostnameManager
import os

logger = MeticulousLogger.getLogger(__name__)

TELEMETRY_QUEUE_PATH = os.getenv(
    "TELEMETRY_QUEUE_PATH", "/meticulous-user/syslog/telemetry/queue"
)


class TelemetryService:
    permissionNotification = None

    @staticmethod
    def onNotificationCallback():
        MeticulousConfig[CONFIG_USER][TELEMETRY_SERVICE_ENABLED] = (
            TelemetryService.permissionNotification.response == NotificationResponse.YES
        )
        opting = "in" if MeticulousConfig[CONFIG_USER][TELEMETRY_SERVICE_ENABLED] else "out"
        capture_message(f"User opted {opting} of telemetry")
        MeticulousConfig.save()

    @staticmethod
    def init():
        if MeticulousConfig[CONFIG_USER][TELEMETRY_SERVICE_ENABLED] is not None:
            return

        logger.info("Sending telemetry notification")
        TelemetryService.permissionNotification = Notification(
            "To improve the performance of all machines, we would like to collect your machine's logs data and sensors information during berwing."
            + "\nWe are asking you to share this data with us as we want all workflows and profile preferences to be optimized for and not just the most common use cases. "
            + "\n\nDo you agree to share this data? (Can be later modified in the advanced settings menu)",
            [NotificationResponse.YES, NotificationResponse.NO],
            image=None,
            callback=TelemetryService.onNotificationCallback,
        )
        NotificationManager.add_notification(TelemetryService.permissionNotification)

    async def upload_queue():
        if not os.path.exists(TELEMETRY_QUEUE_PATH):
            return
        for root, dirnames, filenames in os.walk(TELEMETRY_QUEUE_PATH):
            for file in filenames:
                # validate the symlink is still valid
                file_path = os.path.join(TELEMETRY_QUEUE_PATH, file)
                if os.path.exists(file_path):
                    try:
                        logger.debug(f"sending queued debug file: {file} - [{file_path}]")
                        with open(file_path, mode="rb") as file_data:
                            await TelemetryService.upload_debug_shot(file_data.read(), file)
                        os.remove(file_path)
                    except aiohttp.ClientError as e:
                        logger.warning(f"Error uploading debug file {file_path}: {e}")
                    except Exception as e:
                        logger.warning(
                            f"Error reading debug file {file_path}, removing from queue: {e}"
                        )
                        os.remove(file_path)
                else:
                    logger.warning(f"symbolic link broken: {file_path}, removing from queue")
                    os.remove(file_path)

    # Upload a debug shot file to the server
    async def upload_debug_shot(file_bytes: bytes, filename: str):
        machine_name = HostnameManager.generateHostname()
        url = f"https://analytics.meticulousespresso.com/upload/{machine_name}"
        logger.info(f"Uploading debug shot to {url}")
        data = aiohttp.FormData()
        data.add_field("file", file_bytes, filename=filename)
        config = {"config": MeticulousConfig}
        data.add_field("json", json.dumps(config), content_type="application/json")

        try:
            async with aiohttp.ClientSession(
                timeout=aiohttp.ClientTimeout(total=30)
            ) as session:
                async with session.post(url, data=data) as response:
                    response.raise_for_status()
        except aiohttp.ClientError as e:
            logger.info(f"Upload failed: {e}")
            raise e

        logger.info("Sent debug shot to server")

    @staticmethod
    def track_unsent_shot_file(file_path: str):
        # make the directory part of the file name, its not used in the server anyways
        if not os.path.exists(TELEMETRY_QUEUE_PATH):
            os.makedirs(TELEMETRY_QUEUE_PATH)
        file_directory_path = "_".join(file_path.split(os.path.sep)[-2:])
        LINKED_PATH = os.path.join(TELEMETRY_QUEUE_PATH, file_directory_path)
        try:
            os.symlink(file_path, LINKED_PATH)
        except Exception as e:
            logger.warning(f"cannot track debug file {file_path} for future telemetry: {e}")
