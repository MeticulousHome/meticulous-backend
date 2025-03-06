import json
from notifications import NotificationManager
from .base_handler import BaseHandler
from .api import API, APIVersion

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


class GetNotificationsHandler(BaseHandler):

    def get(self):
        include_acknowledged = (
            self.get_argument("acknowledged", "false").lower() == "true"
        )

        if include_acknowledged:
            # Return all notifications
            notifications = NotificationManager.get_all_notifications()
        else:
            # Return only unacknowledged notifications
            notifications = NotificationManager.get_unacknowledged_notifications()

        self.write(
            json.dumps(
                [
                    {
                        "id": n.id,
                        "message": n.message,
                        "image": n.image,
                        "responses": n.respone_options,
                        "timestamp": n.timestamp.isoformat(),
                    }
                    for n in notifications
                ]
            )
        )

    def post(self):
        data = json.loads(self.request.body)
        notification_id = data.get("id")
        logger.info(f"acknowledge {notification_id}")
        response = data.get("response")
        if NotificationManager.acknowledge_notification(notification_id, response):
            self.write({"status": "success"})
        else:
            self.set_status(404)
            self.write({"status": "failure", "message": "Notification not found"})
            logger.info(
                f"acknoledge failed. Known notifications: {json.dumps([x.id for x in NotificationManager.get_all_notifications()])}"
            )


API.register_handler(APIVersion.V1, r"/notifications", GetNotificationsHandler),
API.register_handler(
    APIVersion.V1, r"/notifications/acknowledge", GetNotificationsHandler
),
