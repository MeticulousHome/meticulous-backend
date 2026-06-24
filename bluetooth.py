import subprocess
from dataclasses import dataclass
from enum import Enum

import sentry_sdk

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


class BluetoothState(str, Enum):
    ON = "on"
    OFF = "off"


@dataclass
class BluetoothStatus:
    powered: bool
    name: str = ""
    address: str = ""
    discoverable: bool = False
    pairable: bool = False

    def to_json(self):
        return {
            "powered": self.powered,
            "name": self.name,
            "address": self.address,
            "discoverable": self.discoverable,
            "pairable": self.pairable,
        }


class BluetoothManager:
    _bluetooth_available = True

    @staticmethod
    def init():
        logger.info("Bluetooth initializing")
        BluetoothManager._bluetooth_available = True
        try:
            BluetoothManager.get_current_status()
        except Exception as e:
            logger.warning(f"Bluetooth unavailable: {e}")
            BluetoothManager._bluetooth_available = False

    @staticmethod
    def run_bluetoothctl_command(command: str):
        try:
            return subprocess.run(
                ["bluetoothctl", *command.split()],
                capture_output=True,
                text=True,
                check=False,
                timeout=10,
            )
        except Exception as e:
            error_msg = f"Error executing bluetoothctl {command}: {e}"
            logger.error(error_msg)
            sentry_sdk.capture_message(error_msg, level="error")
            return None

    @staticmethod
    def get_current_status() -> BluetoothStatus:
        if not BluetoothManager._bluetooth_available:
            return BluetoothStatus(powered=False)

        result = BluetoothManager.run_bluetoothctl_command("show")
        if result is None or result.returncode != 0:
            return BluetoothStatus(powered=False)

        status = {
            "powered": False,
            "name": "",
            "address": "",
            "discoverable": False,
            "pairable": False,
        }

        for line in result.stdout.splitlines():
            if ":" not in line:
                continue
            key, value = [part.strip() for part in line.split(":", 1)]
            match key:
                case "Name":
                    status["name"] = value
                case "Powered":
                    status["powered"] = value.lower() == "yes"
                case "Address":
                    status["address"] = value
                case "Discoverable":
                    status["discoverable"] = value.lower() == "yes"
                case "Pairable":
                    status["pairable"] = value.lower() == "yes"

        return BluetoothStatus(**status)

    @staticmethod
    def set_power_state(state: BluetoothState) -> bool:
        if not BluetoothManager._bluetooth_available:
            return False

        logger.warning(f"Setting Bluetooth power state to {state.value}")
        result = BluetoothManager.run_bluetoothctl_command(f"power {state.value}")
        if result is None or result.returncode != 0:
            return False

        current_status = BluetoothManager.get_current_status()
        return current_status.powered == (state == BluetoothState.ON)
