from log import MeticulousLogger
from notifications import NotificationManager, Notification, NotificationResponse
import subprocess

from dbus_client import AsyncDBUSClient
from api.machine import OSStatus, UpdateOSStatus

notification = None
notification_image = "data:image/jpeg;base64,/9j/4AAQSkZJRgABAQAASABIAAD/4QCMRXhpZgAATU0AKgAAAAgABQEGAAMAAAABAAIAAAEaAAUAAAABAAAASgEbAAUAAAABAAAAUgEoAAMAAAABAAIAAIdpAAQAAAABAAAAWgAAAAAAAABIAAAAAQAAAEgAAAABAAOgAQADAAAAAQABAACgAgAEAAAAAQAAAHigAwAEAAAAAQAAAHgAAAAA/+EKHmh0dHA6Ly9ucy5hZG9iZS5jb20veGFwLzEuMC8APD94cGFja2V0IGJlZ2luPSLvu78iIGlkPSJXNU0wTXBDZWhpSHpyZVN6TlRjemtjOWQiPz4gPHg6eG1wbWV0YSB4bWxuczp4PSJhZG9iZTpuczptZXRhLyIgeDp4bXB0az0iWE1QIENvcmUgNi4wLjAiPiA8cmRmOlJERiB4bWxuczpyZGY9Imh0dHA6Ly93d3cudzMub3JnLzE5OTkvMDIvMjItcmRmLXN5bnRheC1ucyMiPiA8cmRmOkRlc2NyaXB0aW9uIHJkZjphYm91dD0iIiB4bWxuczpJcHRjNHhtcEV4dD0iaHR0cDovL2lwdGMub3JnL3N0ZC9JcHRjNHhtcEV4dC8yMDA4LTAyLTI5LyIgeG1sbnM6ZGM9Imh0dHA6Ly9wdXJsLm9yZy9kYy9lbGVtZW50cy8xLjEvIiBJcHRjNHhtcEV4dDpBcnR3b3JrVGl0bGU9IklNR18wMTUzIj4gPGRjOnRpdGxlPiA8cmRmOkFsdD4gPHJkZjpsaSB4bWw6bGFuZz0ieC1kZWZhdWx0Ij5JTUdfMDE1MzwvcmRmOmxpPiA8L3JkZjpBbHQ+IDwvZGM6dGl0bGU+IDwvcmRmOkRlc2NyaXB0aW9uPiA8L3JkZjpSREY+IDwveDp4bXBtZXRhPiAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAg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"
error_upgrading_OTA_image = "data:image/jpeg;base64,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"
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

        process = "processing OTA deployment" if process == "EPRODEP" else "downloading OTA update"

        logger.error(f"Error in {process} process: {error}")
        
        error_message = f"Oops, something went wrong {process}: [ {error} ]"
        error_hawkbit_notification = Notification(error_message,[NotificationResponse.OK], error_upgrading_OTA_image)
        # dismiss progress notification
        progress_notification.image = ""
        progress_notification.message = ""

        NotificationManager.add_notification(progress_notification)
        NotificationManager.add_notification(error_hawkbit_notification)

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

        if UpdateOSStatus.isRecoveryUpdate():
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
