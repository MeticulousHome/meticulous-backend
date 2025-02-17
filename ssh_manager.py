from pydbus import SystemBus
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


class SSHManager:
    @staticmethod
    def set_ssh_state(enabled: bool) -> bool:
        """
        Enable or disable SSH service using systemd D-Bus interface.

        Args:
            enabled (bool): True to enable and start SSH, False to disable and stop

        Returns:
            bool: True if operation was successful, False otherwise
        """
        try:
            bus = SystemBus()
            systemd = bus.get(".systemd1")

            if enabled:
                # First enable the service
                systemd.EnableUnitFiles(["ssh.service"], False, False)
                # Then start it
                systemd.StartUnit("ssh.service", "replace")
                logger.info("SSH service enabled and started")
            else:
                # First stop the service
                systemd.StopUnit("ssh.service", "replace")
                # Then disable it
                systemd.DisableUnitFiles(["ssh.service"], False)
                logger.info("SSH service stopped and disabled")

            # Reload systemd manager to apply changes
            systemd.Reload()
            return True

        except Exception as e:
            logger.error(f"Error while managing SSH service: {e}")
            return False
