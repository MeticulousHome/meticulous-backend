from pydbus import SystemBus
from gi.repository import GLib
import threading
import subprocess
from dataclasses import dataclass
from log import MeticulousLogger
from notifications import NotificationManager, Notification, NotificationResponse
from queue import Queue, Empty

logger = MeticulousLogger.getLogger(__name__)

@dataclass
class RaucInfo:
    """Class representing the rauc information output."""
    output: str

class RaucManager:
    """Manager class to handle rauc operations."""

    update_messages = Queue()  # Queue to store update messages

    @staticmethod
    def get_rauc_status():
        try:
            result = subprocess.run(["rauc", "status"], stdout=subprocess.PIPE, text=True)
            output = result.stdout

            notification = Notification(
                "RAUC status information retrieved successfully",
                [NotificationResponse.OK]
            )
            NotificationManager.add_notification(notification)
            logger.info("Notification sent: RAUC status information retrieved successfully")

            return RaucInfo(output=output)
        except Exception as e:
            logger.error(f"Error retrieving RAUC status: {str(e)}")
            return RaucInfo(output=f"Error retrieving RAUC status: {str(e)}")

    @staticmethod
    def dbus_message_handler(interface, changed, invalidated, path):
        logger.info(f"DBus message received on interface {interface}, path {path}")
        logger.info(f"Changed properties: {changed}")
        progress = changed.get("Progress", None)
        operation = changed.get("Operation", None)

        if progress:
            message_str = f"Progress: {progress[0]}%, Status: {progress[1]}"
            RaucManager.update_messages.put(message_str)
            logger.info(f"DBus message progress: {message_str}")

        if operation:
            message_str = f"Operation: {operation}"
            RaucManager.update_messages.put(message_str)
            logger.info(f"DBus message operation: {message_str}")
            if operation == "installing":
                notification = Notification(
                    message_str,
                    [NotificationResponse.OK]
                )
                NotificationManager.add_notification(notification)
                logger.info(f"Notification sent: {message_str}")

    @staticmethod
    def start_dbus_monitor():
        try:
            bus = SystemBus()
            bus.subscribe(
                iface="org.freedesktop.DBus.Properties",
                signal="PropertiesChanged",
                arg0="de.pengutronix.rauc.Installer",
                signal_fired=RaucManager.dbus_message_handler
            )
            logger.info("DBus monitor started, waiting for messages...")
            loop = GLib.MainLoop()
            loop.run()
        except Exception as e:
            logger.error(f"Error starting DBus monitor: {str(e)}")

    @staticmethod
    def start_os_update():
        def run_command():
            try:
                # Notify that the update process has started
                start_message = "Update process has started."
                RaucManager.update_messages.put(start_message)
                notification = Notification(
                    start_message,
                    [NotificationResponse.OK]
                )
                NotificationManager.add_notification(notification)
                logger.info("Notification sent: Update process has started.")

                process = subprocess.Popen(
                    ["./update_OS.sh"],
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                    cwd="/home"
                )

                for line in iter(process.stdout.readline, ''):
                    RaucManager.update_messages.put(line.strip())  # Add command output to queue
                    logger.info(f"Update command output: {line.strip()}")

                process.stdout.close()
                process.wait()

            except Exception as e:
                error_message = f"Error executing command: {str(e)}"
                RaucManager.update_messages.put(error_message)
                logger.error(error_message)

        threading.Thread(target=run_command).start()

    @staticmethod
    def get_update_messages():
        messages = []
        while not RaucManager.update_messages.empty():
            messages.append(RaucManager.update_messages.get())
        return RaucInfo(output="\n".join(messages))

# Start the DBUS monitor in a separate thread
dbus_thread = threading.Thread(target=RaucManager.start_dbus_monitor)
dbus_thread.daemon = True
dbus_thread.start()

# Example use within the module
if __name__ == "__main__":
    current_rauc_info = RaucManager.get_rauc_status()
    print(current_rauc_info.output)  # Directly print the output

