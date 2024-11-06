import json
from bluetooth import BluetoothManager, BluetoothState
from .base_handler import BaseHandler
from .api import API, APIVersion
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


class BluetoothStatusHandler(BaseHandler):
    def get(self):
        try:
            status = BluetoothManager.get_current_status()
            self.write(json.dumps(status.to_json()))
        except Exception as e:
            self.set_status(400)
            self.write(
                {"status": "error", "error": f"Failed to get Bluetooth status: {e}"}
            )
            logger.warning(
                "Failed to get Bluetooth status: ", exc_info=e, stack_info=True
            )


class BluetoothPowerHandler(BaseHandler):
    def post(self):
        try:
            data = json.loads(self.request.body)
            state = data.get("state")

            if state not in ["on", "off"]:
                self.set_status(400)
                self.write(
                    {"status": "error", "error": "Invalid state. Use 'on' or 'off'"}
                )
                return

            bt_state = BluetoothState(state.lower())
            success = BluetoothManager.set_power_state(bt_state)

            if success:
                status = BluetoothManager.get_current_status()
                self.write({"status": "ok", "current_state": status.to_json()})
            else:
                self.set_status(400)
                self.write(
                    {
                        "status": "error",
                        "error": f"Failed to set Bluetooth power state to {state}",
                    }
                )

        except json.JSONDecodeError as e:
            self.set_status(400)
            self.write({"status": "error", "error": "Invalid JSON"})
            logger.warning(f"Failed to parse JSON: {e}", stack_info=False)

        except Exception as e:
            self.set_status(400)
            self.write(
                {
                    "status": "error",
                    "error": f"Failed to set Bluetooth power state: {e}",
                }
            )
            logger.warning(
                "Failed to set Bluetooth power state: ", exc_info=e, stack_info=True
            )


API.register_handler(APIVersion.V1, r"/bluetooth/status", BluetoothStatusHandler)
API.register_handler(APIVersion.V1, r"/bluetooth/power", BluetoothPowerHandler)
