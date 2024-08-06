import time
import uuid
import json
import base64
import pyqrcode
import io
import queue
import threading

from sounds import SoundPlayer, Sounds

from config import *

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


class NotificationResponse:
    OK = "Ok"
    YES = "Yes"
    NO = "No"
    UPDATE = "Update"
    SKIP = "Skip"
    ABORT = "Abort"


class Notification:

    def __init__(
        self,
        message,
        responses=[NotificationResponse.OK],
        image=None,
        callback: callable = None,
    ):
        self.id = str(uuid.uuid4())
        self.message = message
        self.respone_options = responses
        self.image = image
        self.acknowledged = False
        self.acknowledged_timestamp = None
        self.response = None
        self.callback = callback
        self.timestamp = datetime.now()

    def add_image(self, filename):
        ext = filename.split(".")[-1]
        prefix = f"data:image/{ext};base64,"
        with open(filename, "rb") as f:
            img = f.read()
        self.image = prefix + base64.b64encode(img).decode("utf-8")

    def add_qrcode(self, qrcontents):
        buffer = io.BytesIO()
        qr = pyqrcode.create(qrcontents)
        qr.png(
            buffer,
            scale=6,
            module_color=[0x00, 0x00, 0x00, 0xFF],
            background=[0xFF, 0xFF, 0xFF, 0xFF],
        )

        prefix = f"data:image/png;base64,"
        self.image = prefix + base64.b64encode(buffer.getvalue()).decode("utf-8")

    def acknowledge(self, response):
        self.acknowledged = True
        self.response = response
        self.acknowledged_timestamp = time.time()
        if self.callback:
            self.callback()

    def to_json(self):
        return json.dumps(
            {
                "id": self.id,
                "message": self.message,
                "image": self.image,
                "responses": [x for x in self.respone_options],
                "timestamp": self.timestamp.isoformat(),
            }
        )


class NotificationManager:
    _notifications = []
    _sio = None
    _queue = queue.Queue()
    _thread = None

    def init(sio):
        NotificationManager._sio = sio
        NotificationManager._thread = threading.Thread(
            target=NotificationManager._notification_loop
        )
        NotificationManager._thread.start()

    def _notification_loop():
        """
        This function runs an asyncio event loop in a new process.
        It continuously checks for new items in the queue and processes them.
        """

        async def process_queue():
            while True:
                try:
                    # Try to get an item from the queue allowing blocking
                    notification: Notification = NotificationManager._queue.get()
                except queue.Empty:
                    # If the queue is empty, wait for a short period before trying again
                    await asyncio.sleep(0.1)
                else:
                    # Emit the notification over socketIO as json
                    await NotificationManager._sio.emit(
                        "notification", notification.to_json()
                    )
                    logger.info(f"send notification: {notification.to_json()}")

        # Create and run the asyncio event loop
        loop = asyncio.new_event_loop()
        asyncio.set_event_loop(loop)
        loop.run_until_complete(process_queue())

    def acknowledge_notification(notification_id, response):
        for notification in NotificationManager.get_unacknowledged_notifications():
            if notification.id == notification_id:
                notification.acknowledge(response)
                if notification.callback:
                    notification.callback(response)
                return True
        return False

    def add_notification(notification: Notification):

        updating = False
        for idx, old_notfication in enumerate(NotificationManager._notifications):
            if (
                notification.id == old_notfication.id
                and not old_notfication.acknowledged
            ):
                del NotificationManager._notifications[idx]
                updating = True

        notification.acknowledged = False
        NotificationManager._notifications.append(notification)
        NotificationManager._queue.put(notification)

        if not updating:
            logger.info("Notification created")
            SoundPlayer.play_event_sound(Sounds.NOTIFICATION)
        else:
            logger.info("Notification updated")

    def get_unacknowledged_notifications():
        NotificationManager.delete_old_acknowledged()
        return [n for n in NotificationManager._notifications if not n.acknowledged]

    def get_all_notifications():
        NotificationManager.delete_old_acknowledged()
        return NotificationManager._notifications

    def delete_old_acknowledged():
        ttl = MeticulousConfig[CONFIG_SYSTEM][NOTIFICATION_KEEPALIVE]
        current_time = time.time()
        NotificationManager._notifications = [
            n
            for n in NotificationManager._notifications
            if not n.acknowledged or (current_time - n.acknowledged_timestamp) < ttl
        ]
