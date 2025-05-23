from log import MeticulousLogger
from notifications import NotificationManager, Notification, NotificationResponse
import subprocess

from dbus_client import AsyncDBUSClient
from api.machine import OSStatus, UpdateOSStatus

notification = None
notification_image = "data:image/jpeg;base64,/9j/4AAQSkZJRgABAQAASABIAAD/4QCMRXhpZgAATU0AKgAAAAgABQEGAAMAAAABAAIAAAEaAAUAAAABAAAASgEbAAUAAAABAAAAUgEoAAMAAAABAAIAAIdpAAQAAAABAAAAWgAAAAAAAABIAAAAAQAAAEgAAAABAAOgAQADAAAAAQABAACgAgAEAAAAAQAAAHigAwAEAAAAAQAAAHgAAAAA/+EKHmh0dHA6Ly9ucy5hZG9iZS5jb20veGFwLzEuMC8APD94cGFja2V0IGJlZ2luPSLvu78iIGlkPSJXNU0wTXBDZWhpSHpyZVN6TlRjemtjOWQiPz4gPHg6eG1wbWV0YSB4bWxuczp4PSJhZG9iZTpuczptZXRhLyIgeDp4bXB0az0iWE1QIENvcmUgNi4wLjAiPiA8cmRmOlJERiB4bWxuczpyZGY9Imh0dHA6Ly93d3cudzMub3JnLzE5OTkvMDIvMjItcmRmLXN5bnRheC1ucyMiPiA8cmRmOkRlc2NyaXB0aW9uIHJkZjphYm91dD0iIiB4bWxuczpJcHRjNHhtcEV4dD0iaHR0cDovL2lwdGMub3JnL3N0ZC9JcHRjNHhtcEV4dC8yMDA4LTAyLTI5LyIgeG1sbnM6ZGM9Imh0dHA6Ly9wdXJsLm9yZy9kYy9lbGVtZW50cy8xLjEvIiBJcHRjNHhtcEV4dDpBcnR3b3JrVGl0bGU9IklNR18wMTUzIj4gPGRjOnRpdGxlPiA8cmRmOkFsdD4gPHJkZjpsaSB4bWw6bGFuZz0ieC1kZWZhdWx0Ij5JTUdfMDE1MzwvcmRmOmxpPiA8L3JkZjpBbHQ+IDwvZGM6dGl0bGU+IDwvcmRmOkRlc2NyaXB0aW9uPiA8L3JkZjpSREY+IDwveDp4bXBtZXRhPiAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAg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"

progress_notification: Notification | None = Notification("")
progress_notification.image = notification_image

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
