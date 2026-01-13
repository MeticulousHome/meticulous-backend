from datetime import datetime
import json
import aiohttp
from sentry_sdk import capture_message

from config import (
    CONFIG_SYSTEM,
    CONFIG_USER,
    MACHINE_DEBUG_SENDING,
    MACHINE_SERIAL_NUMBER,
    MeticulousConfig,
)
from log import MeticulousLogger
from notifications import Notification, NotificationManager, NotificationResponse
from hostname import HostnameManager

logger = MeticulousLogger.getLogger(__name__)


class TelemetryService:
    permissionNotification = None

    @staticmethod
    def onNotificationCallback():
        MeticulousConfig[CONFIG_USER][MACHINE_DEBUG_SENDING] = (
            TelemetryService.permissionNotification.response == NotificationResponse.YES
        )
        opting = "in" if MeticulousConfig[CONFIG_USER][MACHINE_DEBUG_SENDING] else "out"
        capture_message(f"User opted {opting} of telemetry")
        MeticulousConfig.save()

    @staticmethod
    def init():
        current_date = datetime.now()
        if current_date.year > 2025 or (
            current_date.month > 8 and current_date.year == 2025
        ):
            if MeticulousConfig[CONFIG_USER][MACHINE_DEBUG_SENDING]:
                logger.info("Telemetry service is disabled, as testing period is over")
                MeticulousConfig[CONFIG_USER][MACHINE_DEBUG_SENDING] = False
                MeticulousConfig.save()
            else:
                logger.info("Skipping telemetry upload as testing period is over")
            return

        if MeticulousConfig[CONFIG_USER][MACHINE_DEBUG_SENDING] is not None:
            return

        logger.info("Sending telemetry notification")
        TelemetryService.permissionNotification = Notification(
            "To ensure the longevity of all machines, we would like to collect your motors temperature during operation. "
            + "\nWe are asking you to share this data with us as we want all workflows and profile preferences to be optimized for and not just the most common use cases. "
            + "\n\nThis collection will be automatically stopped with the end of the early production testing but latest by August 2025"
            + "\n\nDo you agree to share this data?",
            [NotificationResponse.YES, NotificationResponse.NO],
            image=None,
            callback=TelemetryService.onNotificationCallback,
        )
        NotificationManager.add_notification(TelemetryService.permissionNotification)

    # Upload a debug shot file to the server
    async def upload_debug_shot(file_bytes: bytes, filename: str):
        machine_name = f"{HostnameManager.generateHostname()}-{MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER]}"
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
