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
                systemd.StartUnit("ssh.service", "replace")
                logger.info("SSH service enabled and started")
            else:
                systemd.StopUnit("ssh.service", "replace")
                systemd.DisableUnitFiles(["ssh.service"], False)
                logger.info("SSH service stopped and disabled")

            systemd.Reload()
            return True
        except Exception as e:
            logger.error(f"Error while managing SSH service: {e}")
            return False

    @staticmethod
    def generate_root_password() -> bool:
        try:
            result = subprocess.run(
                ["openssl", "rand", "-base64", "9"],
                capture_output=True,
                text=True,
                check=True,
            )
            new_password = result.stdout.strip()

            # Set the password for root user
            subprocess.run(
                ["chpasswd"], input=f"root:{new_password}".encode(), check=True
            )

            # Store in config for later reference
            MeticulousConfig[CONFIG_SYSTEM][ROOT_PASSWORD] = new_password
            MeticulousConfig.save()

            logger.info("Successfully generated and set new root password")
            return True

        except Exception as e:
            logger.error(f"Error generating or setting root password: {e}")
            return False

    @staticmethod
    def get_root_password() -> str:
        """Get the stored root password from config"""
        return MeticulousConfig[CONFIG_SYSTEM].get(ROOT_PASSWORD)
