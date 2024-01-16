import datetime
import uuid
import json
import asyncio

from dataclasses import dataclass, replace
from enum import Enum, auto, unique

from config import *

@unique
class NotificationResponse(Enum):
    OK = "Ok"
    YES = "Yes"
    NO = "No"
    UPDATE = "Update"
    SKIP = "Skip"

    # Failure type
    UNKNOWN = "Acknowledge"

    @classmethod
    def _missing_(cls, value):
        return cls.UNKNOWN
    
    @classmethod
    def from_str(cls, type_str):
        return cls[type_str.upper()]


class Notification:

    def __init__(self, message, responses=[NotificationResponse.OK], image=None, callback:callable = None):
        self.id = str(uuid.uuid4())
        self.message = message
        self.respone_options=responses
        self.image = image
        self.acknowledged = False
        self.acknowledged_timestamp = None
        self.response = None
        self.callback = callback
        self.timestamp = datetime.now()

    def acknowledge(self, response):
        self.acknowledged = True
        self.response = response
        self.acknowledged_timestamp = datetime.datetime.now()
        if self.callback:
            self.callback()

    def to_json(self):
        return json.dumps({
            "id": self.id,
            "message": self.message,
            "image": self.image,
            "responses": [x.value for x in self.respone_options],
            "timestamp": self.timestamp.isoformat()
        })

class NotificationManager:
    _notifications = []
    _sio = None
    
    def init(sio):
        NotificationManager._sio = sio

    def acknowledge_notification(self, notification_id, response):
        for notification in NotificationManager.get_unacknowledged_notifications():
            if notification.id == notification_id:
                notification.acknowledge(response)
                return True
        return False

    async def add_notification(notification):
        NotificationManager._notifications.append(notification)

        # Emit the notification over socketIO as json
        await NotificationManager._sio.emit("notification", notification.to_json())



    def get_unacknowledged_notifications():
        NotificationManager.delete_old_acknowledged()
        return [n for n in NotificationManager._notifications if not n.acknowledged]
    
    def get_all_notifications():
        NotificationManager.delete_old_acknowledged()
        return NotificationManager._notifications

    def delete_old_acknowledged():
        ttl = MeticulousConfig[CONFIG_SYSTEM][NOTIFICATION_KEEPALIVE]
        current_time = datetime.now()
        NotificationManager._notifications = [
            n for n in NotificationManager._notifications if not n.acknowledged or (current_time - n.acknowledged_timestamp).total_seconds() < ttl
        ]
