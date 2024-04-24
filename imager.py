import os
import threading
import time

from notifications import NotificationManager, Notification, NotificationResponse

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)


class DiscImager:
    src_file = "/meticulous-user/emmc.img"
    dest_file = "/dev/mmcblk2"
    block_size = 1024 * 1024
    copy_thread = None
    notification = None

    @staticmethod
    def flash_if_required(block_size=1024):
        DiscImager.copy_thread = None

        if not os.path.exists(DiscImager.dest_file):
            return
        if not os.path.isfile(DiscImager.src_file):
            return

        logger.info("Starting to image emmc")
        DiscImager.copy_thread = threading.Thread(
            target=DiscImager.copy_file)
        DiscImager.copy_thread.start()

    @staticmethod
    def copy_file():

        waitTime = 10
        time.sleep(waitTime)

        DiscImager.notification = Notification(f"Starting to image emmc in {waitTime} seconds", [
                                               NotificationResponse.OK, NotificationResponse.SKIP])
        NotificationManager.add_notification(DiscImager.notification)
        DiscImager.notification.respone_options = [NotificationResponse.ABORT]
        time.sleep(waitTime)
        last_reported_progress = 0.0
        if DiscImager.notification.acknowledged and DiscImager.notification.response == NotificationResponse.SKIP:
            return
        try:
            with open(DiscImager.src_file, 'rb') as src, open(DiscImager.dest_file, 'wb') as dest:
                src_size = os.stat(DiscImager.src_file).st_size
                copied = 0

                while True:
                    if DiscImager.notification.acknowledged and DiscImager.notification.response == NotificationResponse.ABORT:
                        NotificationManager.add_notification(
                            Notification("Flashing Aborted"))
                        return

                    data = src.read(DiscImager.block_size)
                    if not data:
                        break

                    dest.write(data)
                    copied += len(data)
                    progress = (copied / src_size) * 100
                    if progress >= last_reported_progress + 0.1:
                        logger.info(f"Progress: {progress:.1f}%")
                        DiscImager.notification.message = f"Progress: {progress: .1f} %"
                        NotificationManager.add_notification(
                            DiscImager.notification)
                        last_reported_progress = progress

                logger.info(
                    f"\nFile '{DiscImager.src_file}' copied to '{DiscImager.dest_file}'")

                DiscImager.notification.message = f"Flashing finished. Remove the boot jumper and reset the machine"
                DiscImager.notification.respone_options = [
                    NotificationResponse.OK]

                NotificationManager.add_notification(
                    DiscImager.notification)
        except IOError as e:
            logger.info(f"Error: {e.strerror}")
