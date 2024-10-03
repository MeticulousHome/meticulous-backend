from machine import Machine
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


class HeaterActuator:
    @staticmethod
    def set_timeout(timeout_minutes):
        if not isinstance(timeout_minutes, int):
            raise ValueError("Timeout must be an integer")
        if timeout_minutes < 0 or timeout_minutes > 60:  # 1 hour at most
            raise ValueError("Timeout must be between 0 and 60 minutes")

        command = f"heater_timeout,{timeout_minutes:d}\x03"
        logger.info(f"Setting heater timeout on ESP32: {timeout_minutes} minutes")
        Machine.write(command.encode("utf-8"))
