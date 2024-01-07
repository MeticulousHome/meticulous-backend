import tornado.ioloop
import tornado.web
import json
from notifications import NotificationManager
from .base_handler import BaseHandler

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)

class GetNotificationsHandler(BaseHandler):

    def get(self):
        unacknowledged_only = self.get_argument("acknowledged", "false").lower() == "true"
        
        if unacknowledged_only:
            # Return only unacknowledged notifications
            notifications = NotificationManager.get_unacknowledged_notifications()
        else:
            # Return all notifications
            notifications = NotificationManager.get_all_notifications()

        self.write(json.dumps(
            [
                {"id": n.id, "message": n.message, "image": n.image, "timestamp": n.timestamp.isoformat()} for n in notifications
            ]))

class AcknowledgeNotificationHandler(BaseHandler):

    def post(self):
        data = json.loads(self.request.body)
        notification_id = data.get("id")
        response = data.get("response")
        if NotificationManager.acknowledge_notification(notification_id, response):
            self.write({"status": "success"})
        else:
            self.write({"status": "failure", "message": "Notification not found"})

NOTIFICATIONS_HANDLER = [
        (r"/notifications", GetNotificationsHandler),
        (r"/notifications/acknowledge", AcknowledgeNotificationHandler),
    ]