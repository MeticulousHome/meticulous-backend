import subprocess
from enum import Enum
from dataclasses import dataclass
from typing import Dict, Optional
import sentry_sdk

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


class BluetoothState(str, Enum):
    ON = "on"
    OFF = "off"
    UNKNOWN = "unknown"


@dataclass
class BluetoothStatus:
    """Class representing the current Bluetooth configuration"""

    powered: bool
    name: str
    address: str
    discoverable: bool
    pairable: bool

    def to_json(self) -> Dict:
        return {
            "powered": self.powered,
            "name": self.name,
            "address": self.address,
            "discoverable": self.discoverable,
            "pairable": self.pairable,
        }


class BluetoothManager:
    _thread = None
    _bluetooth_available = True

    @staticmethod
    def run_bluetoothctl_command(command: str) -> Optional[str]:
        """Run a bluetoothctl command and return the output as a string."""
        try:
            result = subprocess.run(
                ["bluetoothctl"] + command.split(),
                capture_output=True,
                text=True,
                check=True,
            )
            return result.stdout
        except subprocess.CalledProcessError as e:
            error_msg = f"Error executing '{command}': {e}"
            logger.error(error_msg)
            sentry_sdk.capture_message(error_msg, level="error")
            return None

    @staticmethod
    def init():
        logger.info("Bluetooth initializing")
        try:
            BluetoothManager.get_current_status()
        except Exception as e:
            logger.warning(f"Bluetooth unavailable! {e}")
            BluetoothManager._bluetooth_available = False

    @staticmethod
    def get_current_status() -> BluetoothStatus:
        """Get current Bluetooth status using bluetoothctl show."""
        if not BluetoothManager._bluetooth_available:
            return BluetoothStatus(
                powered=False, name="", address="", discoverable=False, pairable=False
            )

        output = BluetoothManager.run_bluetoothctl_command("show")
        if output is None:
            return BluetoothStatus(
                powered=False, name="", address="", discoverable=False, pairable=False
            )

        status = {
            "powered": False,
            "name": "",
            "address": "",
            "discoverable": False,
            "pairable": False,
        }

        for line in output.splitlines():
            if "Name" in line:
                status["name"] = line.split(": ")[1].strip()
            elif "Powered" in line:
                status["powered"] = "yes" in line.lower()
            elif "Address" in line:
                status["address"] = line.split(": ")[1].strip()
            elif "Discoverable" in line:
                status["discoverable"] = "yes" in line.lower()
            elif "Pairable" in line:
                status["pairable"] = "yes" in line.lower()

        return BluetoothStatus(**status)

    @staticmethod
    def set_power_state(state: BluetoothState) -> bool:
        """Set Bluetooth power state and verify the change."""
        if not BluetoothManager._bluetooth_available:
            return False

        logger.info(f"Setting Bluetooth power state to: {state}")

        command = f"power {state.value}"
        BluetoothManager.run_bluetoothctl_command(command)

        current_status = BluetoothManager.get_current_status()
        target_powered = state == BluetoothState.ON

        if current_status.powered == target_powered:
            logger.info(f"Bluetooth successfully turned {state.value}")
            return True
        else:
            error_msg = f"Failed to turn Bluetooth {state.value}"
            logger.error(error_msg)
            sentry_sdk.capture_message(error_msg, level="error")
            return False
