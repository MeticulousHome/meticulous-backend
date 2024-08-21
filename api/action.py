import json
from machine import Machine
from .base_handler import BaseHandler
from .api import API, APIVersion

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


class ExecuteActionHandler(BaseHandler):
    def post(self, action):
        BACKEND_ACTIONS = ["reset"]
        ESP_ACTIONS = ["start", "stop", "tare"]
        allowed_actions = BACKEND_ACTIONS + ESP_ACTIONS
        if action in ESP_ACTIONS:
            success = Machine.action(action_event=action)
            if success:
                self.write(json.dumps({"action": action, "status": "ok"}))
            else:
                self.set_status(409)
                self.write(json.dumps({"status": "error", "action": action}))
        elif action in BACKEND_ACTIONS:
            match action:
                case "reset":
                    Machine.reset()
                    self.write(json.dumps({"action": action, "status": "ok"}))
        else:
            self.set_status(400)
            self.write(
                json.dumps(
                    {
                        "status": "error",
                        "action": action,
                        "allowed_actions": allowed_actions,
                    }
                )
            )

    def get(self, action):
        return self.post(action)


API.register_handler(APIVersion.V1, r"/action/(.*)", ExecuteActionHandler),
