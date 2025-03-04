from config import MeticulousConfig, CONFIG_USER, MACHINE_DEBUG_SENDING
from notifications import NotificationManager, Notification, NotificationResponse
from datetime import datetime
from log import MeticulousLogger
from sentry_sdk import capture_message

logger = MeticulousLogger.getLogger(__name__)


class TelemetryService:

    @staticmethod
    def onNotificationCallback(response):
        MeticulousConfig[CONFIG_USER][MACHINE_DEBUG_SENDING] = (
            response == NotificationResponse.YES
        )
        capture_message(
            f"User opted {"in" if MeticulousConfig[CONFIG_USER][MACHINE_DEBUG_SENDING] else "out"} of telemetry"
        )
        MeticulousConfig.save()

    @staticmethod
    def init():
        current_date = datetime.now()
        if (
            MeticulousConfig[CONFIG_USER][MACHINE_DEBUG_SENDING]
            and current_date.year > 2025
            or (current_date.month > 8 and current_date.year == 2025)
        ):
            logger.info("Telemetry service is disabled, as testing period is over")
            MeticulousConfig[CONFIG_USER][MACHINE_DEBUG_SENDING] = False
            MeticulousConfig.save()
            return

        if MeticulousConfig[CONFIG_USER][MACHINE_DEBUG_SENDING] is not None:
            return

        logger.info("Sending telemetry notification")
        updateNotification = Notification(
            "To ensure the longevity of all machines, we would like to collect your motors temperature during operation. "
            + "\nWe are asking you to share this data with us as we want all workflows and profile preferences to be optimized for and not just the most common use cases. "
            + "\n\nThis collection will be automatically stopped with the end of the early production testing but latest by August 2025"
            + "\n\nDo you agree to share this data?",
            [NotificationResponse.YES, NotificationResponse.NO],
            image=None,
            callback=TelemetryService.onNotificationCallback,
        )
        NotificationManager.add_notification(updateNotification)
