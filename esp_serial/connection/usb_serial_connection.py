import serial
import serial.tools.list_ports
from esptool.reset import DEFAULT_RESET_DELAY, ClassicReset, HardReset

from log import MeticulousLogger

from .serial_connection import SerialConnection

logger = MeticulousLogger.getLogger(__name__)


class USBSerialConnection(SerialConnection):
    # Uses the parent constructor

    def reset(self, bootloader=False, sleep=DEFAULT_RESET_DELAY, ignored_bootloader_sleep=0):
        logger.info("Resetting ESP32")
        if bootloader:
            ClassicReset(self.port, reset_delay=sleep)
        else:
            HardReset(self.port)

    def connect(self, device=None):
        if device is None:
            device = []
            ports = serial.tools.list_ports.comports()
            for port, desc, hwid in sorted(ports):
                logger.debug(f"{port}: {desc} [{hwid}]")
                if "ttyUSB" in port or "ttyS" in port:
                    device.append(port)
        logger.info(f"Connecting to USB port(s): {device}")
        return super().connect(device)
