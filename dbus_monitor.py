from log import MeticulousLogger
from notifications import NotificationManager, Notification, NotificationResponse
import subprocess

from config import MeticulousConfig, CONFIG_MANUFACTURING
from hostname import HostnameManager
import os
import time
from manufacturing import LAST_BOOT_MODE_KEY

from dbus_client import AsyncDBUSClient
from api.machine import OSStatus, UpdateOSStatus
from images.notificationImages.base64 import (
    FLASHING_NOTIFICATION_IMAGE as notification_image,
    USB_DEVICE_NOTIFICATION_IMAGE as usb_device_notification_image,
)

notification = None

progress_notification: Notification | None = Notification("")
usb_device_notification: Notification | None = Notification("")
progress_notification.image = notification_image
usb_device_notification.image = usb_device_notification_image

error_rauc_updating = ""

logger = MeticulousLogger.getLogger(__name__)


class DBusMonitor:

    dbus_object = AsyncDBUSClient()

    @classmethod
    def init(self):
        self.dbus_object.new_signal_subscription(
            "com.Meticulous.Handler.Updater",
            "UpdateFailed",
            self.recovery_update_failed,
        )

        self.dbus_object.new_signal_subscription(
            "de.pengutronix.rauc.Installer", "Completed", self.rauc_update_complete
        )

        self.dbus_object.new_signal_subscription(
            "com.Meticulous.Handler.MassStorage", "NewUSB", self.notify_usb
        )

        # signal to identify the OS update is from the USB
        self.dbus_object.new_signal_subscription(
            "com.Meticulous.Handler.MassStorage", "RecoveryUpdate", self.recovery_update
        )

        self.dbus_object.new_signal_subscription(
            "org.hawkbit.DownloadProgress",
            "ProgressUpdate",
            self.download_progress,
        )

        self.dbus_object.new_signal_subscription(
            "org.hawkbit.DownloadProgress",
            "Error",
            self.report_hawkbit_error,
        )

        self.dbus_object.new_property_subscription(
            "de.pengutronix.rauc.Installer", "Progress", self.install_progress
        )
        self.dbus_object.new_property_subscription(
            "de.pengutronix.rauc.Installer", "LastError", self.report_error
        )
        self.dbus_object.start()

    @classmethod
    def enableUSBTest(self):
        if (
            MeticulousConfig[CONFIG_MANUFACTURING][LAST_BOOT_MODE_KEY]
            == "manufacturing"
        ):
            logger.info("subscribing to usb test signal on dbus")
            self.dbus_object.new_signal_subscription(
                "com.Meticulous.Handler.MassStorage",
                "NewUSB",
                self.notify_usb_test,
            )

    @staticmethod
    async def download_progress(
        connection,
        sender_name,
        object_path,
        interface_name,
        signal_name,
        parameters: tuple,
    ):
        percentage = parameters[0]

        UpdateOSStatus.sendStatus(OSStatus.DOWNLOADING, round(percentage), None)

        if UpdateOSStatus.isRecoveryUpdate():
            progress_notification.message = f"Downloading update: {percentage}%"
            progress_notification.respone_options = [NotificationResponse.OK]
            progress_notification.image = notification_image
            NotificationManager.add_notification(progress_notification)

    @staticmethod
    async def report_hawkbit_error(
        connection,
        sender_name,
        object_path,
        interface_name,
        signal_name,
        parameters: tuple,
    ):
        process: str = parameters[0]
        error: str = parameters[1]

        process = "processing deployment" if process == "EPRODEP" else "downloading"

        logger.error(f"Error in {process} process: {error}")

    @staticmethod
    async def install_progress(
        connection,
        sender_name,
        object_path,
        property_interface,
        attribute,
        status: tuple[int, str, int],
    ):
        (progress, message, depth) = status
        progress_notification.message = f"Updating OS:\n {progress}%"
        progress_notification.respone_options = [NotificationResponse.OK]
        progress_notification.image = notification_image

        UpdateOSStatus.sendStatus(OSStatus.INSTALLING, progress, None)

        if UpdateOSStatus.isRecoveryUpdate():
            NotificationManager.add_notification(progress_notification)

    @staticmethod
    async def report_error(
        connection, sender_name, object_path, property_interface, attribute, status
    ):
        global error_rauc_updating
        error_rauc_updating = status
        if status == "":
            return
        notification_message = f"There was an error updating the OS:\n {status}"

        UpdateOSStatus.sendStatus(OSStatus.FAILED, 0, status)

        if UpdateOSStatus.isRecoveryUpdate():
            # dismiss progress notification
            progress_notification.image = ""
            progress_notification.message = ""

            NotificationManager.add_notification(progress_notification)

            NotificationManager.add_notification(
                Notification(
                    message=notification_message,
                    responses=[NotificationResponse.OK],
                    image=notification_image,
                )
            )

        UpdateOSStatus.markAsRecoveryUpdate(False)

        subprocess_result = subprocess.run(
            "umount /tmp/possible_updater", shell=True, capture_output=True
        )
        logger.warning(f"{subprocess_result}")

        subprocess_result = subprocess.run(
            "rm -r /tmp/possible_updater", shell=True, capture_output=True
        )
        logger.warning(f"{subprocess_result}")

    @staticmethod
    async def rauc_update_complete(
        connection, sender_name, object_path, interface_name, signal_name, parameters
    ):

        UpdateOSStatus.sendStatus(OSStatus.COMPLETE, 100, None)

        if error_rauc_updating != "":
            notification_message = f"Failed OS updated no need to reboot your machine\n Error: {error_rauc_updating}"
            logger.info(f"error is [{error_rauc_updating}]")
        else:
            notification_message = "OS updated. Remove USB and reboot your machine"

        # dismiss progress notification
        if UpdateOSStatus.isRecoveryUpdate():
            progress_notification.image = ""
            progress_notification.message = ""

            NotificationManager.add_notification(progress_notification)

            NotificationManager.add_notification(
                Notification(
                    message=notification_message,
                    responses=[NotificationResponse.OK],
                    image=notification_image,
                )
            )

        UpdateOSStatus.markAsRecoveryUpdate(False)

        subprocess_result = subprocess.run(
            "umount /tmp/possible_updater", shell=True, capture_output=True
        )
        logger.warning(f"{subprocess_result}")

        subprocess_result = subprocess.run(
            "rm -r /tmp/possible_updater", shell=True, capture_output=True
        )
        logger.warning(f"{subprocess_result}")

    @staticmethod
    async def just_print(
        self,
        connection,
        sender_name,
        object_path,
        property_interface,
        attribute,
        status,
    ):
        logger.info(f"property: [{attribute}], is [{status}]")

    @staticmethod
    async def notify_usb(
        connection, sender_name, object_path, interface_name, signal_name, parameters
    ):

        logger.info(f"received signal NEW USB with parameters: [{parameters}]")
        USB_PATH = parameters[0]

        logger.info(f"USB PATH RECEIVED: {USB_PATH}")

    @staticmethod
    async def notify_usb_test(
        connection, sender_name, object_path, interface_name, signal_name, parameters
    ):
        USB_PATH = parameters[0]
        logger.info(f"USB Device connected, {USB_PATH}")
        usb_device_notification.message = "USB valid"
        usb_device_notification.respone_options = [NotificationResponse.OK]
        MOUNT_PATH = "/tmp/test_device"
        os.makedirs(MOUNT_PATH, exist_ok=True)
        try:
            subprocess.run(["mount", USB_PATH, MOUNT_PATH], check=True)
            machine_name = HostnameManager.generateHostname()
            test_path = os.path.join(MOUNT_PATH, machine_name)
            with open(test_path, "w") as f:
                f.write(machine_name)

            time.sleep(2)
            with open(test_path, "r") as f:
                read_str = f.read()
            if read_str == machine_name:
                NotificationManager.add_notification(usb_device_notification)
        except subprocess.CalledProcessError as e:
            logger.warning(f"Cannot mount '{USB_PATH}' device: {e}")
        except Exception as e:
            logger.warning(f"error testing '{USB_PATH}' device: {e}")

        try:
            subprocess.run(["umount", MOUNT_PATH], check=True)
        except subprocess.CalledProcessError as e:
            logger.warning(f"error unmounting '{MOUNT_PATH}': {e}")

    @staticmethod
    async def recovery_update_failed(
        connection, sender_name, object_path, interface_name, signal_name, parameters
    ):
        error_message: str = (
            f"Recovery Update Failed:\n {parameters[0] if parameters[0] != 'unknown' else 'unknown error, possible USB disconnection'}"
        )

        progress_notification.image = ""
        progress_notification.message = ""

        UpdateOSStatus.sendStatus(OSStatus.FAILED, 0, parameters[0])

        NotificationManager.add_notification(progress_notification)

        NotificationManager.add_notification(
            Notification(
                message=error_message,
                responses=[NotificationResponse.OK],
                image=notification_image,
            )
        )
        UpdateOSStatus.markAsRecoveryUpdate(False)

        logger.info(f"RECOVERY UPDATE FAILED: {error_message}")

    @staticmethod
    async def recovery_update(
        connection, sender_name, object_path, interface_name, signal_name, parameters
    ):

        UpdateOSStatus.markAsRecoveryUpdate(True)

        logger.info("Update in course is a recovery update")
