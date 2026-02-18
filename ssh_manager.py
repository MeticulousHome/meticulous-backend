import subprocess
import re
from config import MeticulousConfig, CONFIG_SYSTEM, ROOT_PASSWORD
from machine import Machine
from system_services import SystemServices

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


class SSHManager:
    ISSUE_PATH = "/etc/issue"

    @staticmethod
    def set_ssh_state(enabled: bool) -> bool:
        """
        Enable or disable SSH service using systemd D-Bus interface.
        """
        return SystemServices.set_service_state("ssh.service", enabled)

    @staticmethod
    def init():

        # First of all, I get the password from the current configuration
        current_password = SSHManager.get_root_password()

        logger.info(
            f"Initializing SSH manager... Manufactoring mode: {Machine.enable_manufacturing}. Password already set: {current_password is not None}"
        )

        # Dont mess with the SSH service if we are emulating
        if Machine.emulated:
            return

        # This depends on the machine class to have initialized. If a serial is in the config it means the machine
        # has been initialized and we can set a password. If the ESP does know its serial but the config is empty
        # we assume the machine was reset and set a new password on next boot
        if not Machine.enable_manufacturing and current_password is None:
            logger.info("Detected exit from manufacturing mode and generating password")
            if SSHManager.generate_root_password():
                logger.warning("Root password generated successfully")
                # I update this value with the newly generated password
                current_password = SSHManager.get_root_password()
            else:
                logger.error("Root password generation failed")

        if current_password is not None:
            SSHManager.update_issue_file(current_password)
            logger.info("Updated /etc/issue with current root password")

            SSHManager.set_root_password(current_password)
            logger.info("The root password matches the configuration")

    @staticmethod
    def generate_root_password() -> bool:
        try:
            new_password = SSHManager.generate_random_password()
            SSHManager.set_root_password(new_password)

            MeticulousConfig[CONFIG_SYSTEM][ROOT_PASSWORD] = new_password
            MeticulousConfig.save()

            logger.info("Successfully generated and set new root password")
            return True
        except Exception as e:
            logger.error(f"Error generating or setting root password:{e}")
            return False

    @staticmethod
    def generate_random_password() -> str:
        # -s or --secure
        #     Generate completely random passwords
        # -B or --ambiguous
        #     Don't include ambiguous characters in the password
        result = subprocess.run(
            ["pwgen", "-s", "-B", "-1", "9"],
            capture_output=True,
            text=True,
            check=True,
        )
        return result.stdout.strip()

    @staticmethod
    def set_root_password(password: str) -> bool:
        try:
            subprocess.run(["chpasswd"], input=f"root:{password}".encode(), check=True)
            return True
        except Exception as e:
            logger.error(f"Error setting the root password: {e}")
            return False

    @staticmethod
    def get_root_password() -> str:
        """Get the stored root password from config"""
        return MeticulousConfig[CONFIG_SYSTEM][ROOT_PASSWORD]

    @staticmethod
    def update_issue_file(password: str) -> bool:
        try:
            password_info = f"\nRoot password: {password}\n\n"
            try:
                with open(SSHManager.ISSUE_PATH, "r") as issue_file:
                    content = issue_file.read()
                pattern = r"\nRoot password: .+\n\n"
                if re.search(pattern, content):
                    content = re.sub(pattern, password_info, content)
                else:
                    content += password_info
            except FileNotFoundError:
                content = password_info
            with open(SSHManager.ISSUE_PATH, "w") as issue_file:
                issue_file.write(content)

            logger.info(
                f"Successfully updated {SSHManager.ISSUE_PATH} with root password"
            )
            return True
        except Exception as e:
            logger.error(f"Error updating {SSHManager.ISSUE_PATH} file: {e}")
            return False
