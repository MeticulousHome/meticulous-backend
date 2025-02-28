from pydbus import SystemBus
from log import MeticulousLogger
import subprocess
from config import MeticulousConfig, CONFIG_SYSTEM, ROOT_PASSWORD

logger = MeticulousLogger.getLogger(__name__)


class SSHManager:
    @staticmethod
    def set_ssh_state(enabled: bool) -> bool:
        """
        Enable or disable SSH service using systemd D-Bus interface.
        """
        try:
            bus = SystemBus()
            systemd = bus.get(".systemd1")

            if enabled:
                systemd.EnableUnitFiles(["ssh.service"], False, False)
                systemd.StartUnit("ssh.service", "fail")
                logger.info("SSH service enabled and started")
            else:
                systemd.StopUnit("ssh.service", "fail")
                systemd.DisableUnitFiles(["ssh.service"], False)
                logger.info("SSH service stopped and disabled")

            systemd.Reload()
            return True
        except Exception as e:
            logger.error(f"Error while managing SSH service: {e}")
            return False

    @staticmethod
    def handle_manufacturing_mode_exit():
        manufacturing_config = MeticulousConfig.get("manufacturing", {})

        if not manufacturing_config.get("enable", True) and manufacturing_config.get(
            "first_boot", True
        ):
            logger.info("Detected exit from manufacturing mode and generating password")
            if SSHManager.generate_root_password():
                manufacturing_config["first_boot"] = False
                MeticulousConfig["manufacturing"] = manufacturing_config
                MeticulousConfig.save()
                logger.info("Root password generated successfully")
            else:
                logger.error("Root password generation failed")

        return manufacturing_config

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
        result = subprocess.run(
            ["openssl", "rand", "-base64", "9"],
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
        return MeticulousConfig[CONFIG_SYSTEM].get(ROOT_PASSWORD)
