from pydbus import SystemBus

from machine import Machine
from config import (
    MeticulousConfig,
    CONFIG_USER,
    SSH_ENABLED,
    TELEMETRY_SERVICE_ENABLED,
)

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


class SystemServices:

    @staticmethod
    def set_service_state(service: str, enabled: bool) -> bool:
        """
        Enable or disable a systemd service using the D-Bus interface.
        """
        try:
            bus = SystemBus()
            systemd = bus.get(".systemd1")

            if enabled:
                systemd.EnableUnitFiles([service], False, False)
                systemd.StartUnit(service, "fail")
                logger.info(f"{service} enabled and started")
            else:
                systemd.StopUnit(service, "fail")
                systemd.DisableUnitFiles([service], False)
                logger.info(f"{service} stopped and disabled")

            systemd.Reload()
            return True
        except Exception as e:
            logger.error(f"Error while managing {service}: {e}")
            return False

    @staticmethod
    def init():
        """
        Sync systemd service states from config on boot.
        Config is the single source of truth.
        """
        if Machine.emulated:
            logger.info("Skipping system services init in emulated mode")
            return

        ssh_enabled = MeticulousConfig[CONFIG_USER].get(SSH_ENABLED, True)
        logger.info(f"Syncing ssh.service state: {'enabled' if ssh_enabled else 'disabled'}")
        SystemServices.set_service_state("ssh.service", ssh_enabled)

        telemetry_enabled = MeticulousConfig[CONFIG_USER].get(TELEMETRY_SERVICE_ENABLED, True)
        logger.info(f"Syncing fluent-bit.service state: {'enabled' if telemetry_enabled else 'disabled'}")
        SystemServices.set_service_state("fluent-bit.service", telemetry_enabled)
