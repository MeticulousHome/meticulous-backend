import subprocess
from dataclasses import dataclass
from log import MeticulousLogger
from notifications import NotificationManager, Notification, NotificationResponse
from queue import Queue

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
    def start_os_update():
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

            # Log before starting the process
            logger.info("Starting update_OS.sh script")

            notification = Notification(
                "Starting update_OS.sh script",
                [NotificationResponse.OK]
            )
            NotificationManager.add_notification(notification)
            logger.info("Notification sent: Starting update_OS.sh script")

            process = subprocess.Popen(
                ["./update_OS.sh"],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                cwd="/home"
            )

            stdout, stderr = process.communicate()

            for line in stdout.splitlines():
                RaucManager.update_messages.put(line.strip())  # Add command output to queue
                logger.info(f"Update command output: {line.strip()}")

            if stderr:
                for line in stderr.splitlines():
                    RaucManager.update_messages.put(line.strip())  # Add error output to queue
                    logger.error(f"Update command error: {line.strip()}")

            # Log after process completes
            logger.info("update_OS.sh script completed")

            if process.returncode != 0:
                error_message = f"update_OS.sh script failed with return code {process.returncode}"
                RaucManager.update_messages.put(error_message)
                logger.error(error_message)

        except Exception as e:
            error_message = f"Error executing command: {str(e)}"
            RaucManager.update_messages.put(error_message)
            logger.error(error_message)

    @staticmethod
    def get_update_messages():
        messages = []
        while not RaucManager.update_messages.empty():
            messages.append(RaucManager.update_messages.get())
        return RaucInfo(output="\n".join(messages))

# Example use within the module
if __name__ == "__main__":
    current_rauc_info = RaucManager.get_rauc_status()
    print(current_rauc_info.output)  # Directly print the output

