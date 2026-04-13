import json
import aiohttp
from sentry_sdk import capture_message

from config import (
    CONFIG_USER,
    TELEMETRY_SERVICE_ENABLED,
    MACHINE_DEBUG_SENDING,
    MeticulousConfig,
    DEBUG_HISTORY_PATH,
)
from log import MeticulousLogger
from notifications import Notification, NotificationManager, NotificationResponse
from hostname import HostnameManager
import os
from shot_database import ShotDataBase

logger = MeticulousLogger.getLogger(__name__)

TELEMETRY_QUEUE_PATH = os.getenv(
    "TELEMETRY_QUEUE_PATH", "/meticulous-user/syslog/telemetry/queue"
)


class TelemetryService:
    telemetryPermissionNotification = None
    debugPermissionNotification = None

    @staticmethod
    def onNotificationCallback_telemetry():
        MeticulousConfig[CONFIG_USER][TELEMETRY_SERVICE_ENABLED] = (
            TelemetryService.telemetryPermissionNotification.response
            == NotificationResponse.YES
        )
        opting = "in" if MeticulousConfig[CONFIG_USER][TELEMETRY_SERVICE_ENABLED] else "out"
        capture_message(f"User opted {opting} of machine telemetry")
        MeticulousConfig.save()

    @staticmethod
    def onNotificationCallback_debug():
        MeticulousConfig[CONFIG_USER][MACHINE_DEBUG_SENDING] = (
            TelemetryService.debugPermissionNotification.response == NotificationResponse.YES
        )
        opting = "in" if MeticulousConfig[CONFIG_USER][MACHINE_DEBUG_SENDING] else "out"
        capture_message(f"User opted {opting} of shot debug data telemetry")
        MeticulousConfig.save()

    @staticmethod
    def init():
        if MeticulousConfig[CONFIG_USER][TELEMETRY_SERVICE_ENABLED] is None:
            logger.info("Sending telemetry notification")
            TelemetryService.telemetryPermissionNotification = Notification(
                "We'd like your permission to collect system journal logs from your Meticulous device. This data helps us debug and fix issues, improve performance, and optimize workflows during the product lifetime"
                + "\n\nDo you agree to share this data? You can change this setting later in Advanced Settings",
                [NotificationResponse.YES, NotificationResponse.NO],
                image=None,
                callback=TelemetryService.onNotificationCallback_telemetry,
            )
            NotificationManager.add_notification(
                TelemetryService.telemetryPermissionNotification
            )

        if MeticulousConfig[CONFIG_USER][MACHINE_DEBUG_SENDING] is None:
            logger.info("Sending shot debug data telemetry notification")
            TelemetryService.debugPermissionNotification = Notification(
                "We'd like your permission to collect debug data from your Meticulous device brews."
                + "\nThis data helps us debug and fix issues, improve performance and your experience with the machine"
                + "\n\nDo you agree to share this data? You can change this setting later in Advanced Settings",
                [NotificationResponse.YES, NotificationResponse.NO],
                image=None,
                callback=TelemetryService.onNotificationCallback_debug,
            )
            NotificationManager.add_notification(TelemetryService.debugPermissionNotification)

    async def upload_queue():
        filenames = ShotDataBase.fetch_debug_files_to_send()
        successful_sent_files: list[str] = []
        for file in filenames:
            # validate the symlink is still valid
            file_path = os.path.join(DEBUG_HISTORY_PATH, file)
            if os.path.exists(file_path):
                try:
                    logger.debug(f"sending queued debug file: {file}")
                    with open(file_path, mode="rb") as file_data:
                        await TelemetryService.upload_debug_shot(file_data.read(), file)
                    successful_sent_files.append(file)
                except aiohttp.ClientError as e:
                    logger.warning(f"Error uploading debug file {file_path}: {e}")
                except Exception:
                    logger.warning(f"Error reading debug file {file_path}")
            else:
                logger.warning(f"file not found: {file_path}, unlinking debug file")
                ShotDataBase.unlink_debug_file(file)

        ShotDataBase.mark_debug_files_sent(successful_sent_files)

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
