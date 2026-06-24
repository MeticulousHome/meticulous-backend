import json

from bluetooth import BluetoothManager, BluetoothState
from log import MeticulousLogger

from .api import API, APIVersion
from .base_handler import BaseHandler

logger = MeticulousLogger.getLogger(__name__)


class BluetoothStatusHandler(BaseHandler):
    def get(self):
        try:
            self.write(json.dumps(BluetoothManager.get_current_status().to_json()))
        except Exception as e:
            self.set_status(400)
            self.write({"status": "error", "error": f"Failed to get Bluetooth status: {e}"})
            logger.warning("Failed to get Bluetooth status", exc_info=e, stack_info=True)


class BluetoothPowerHandler(BaseHandler):
    def post(self):
        try:
            data = json.loads(self.request.body)
            state = data.get("state")
            if state not in ["on", "off"]:
                self.set_status(400)
                self.write({"status": "error", "error": "Invalid state. Use 'on' or 'off'."})
                return

            success = BluetoothManager.set_power_state(BluetoothState(state))
            status = BluetoothManager.get_current_status()
            if not success:
                self.set_status(400)
                self.write(
                    {
                        "status": "error",
                        "error": f"Failed to set Bluetooth power state to {state}",
                        "current_state": status.to_json(),
                    }
                )
                return

            self.write({"status": "ok", "current_state": status.to_json()})
        except json.JSONDecodeError as e:
            self.set_status(400)
            self.write({"status": "error", "error": "Invalid JSON"})
            logger.warning(f"Failed to parse JSON: {e}", stack_info=False)
        except Exception as e:
            self.set_status(400)
            self.write({"status": "error", "error": f"Failed to set Bluetooth power state: {e}"})
            logger.warning("Failed to set Bluetooth power state", exc_info=e, stack_info=True)


API.register_handler(APIVersion.V1, r"/bluetooth/status", BluetoothStatusHandler)
API.register_handler(APIVersion.V1, r"/bluetooth/power", BluetoothPowerHandler)
