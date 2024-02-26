import json
from machine import Machine
from .base_handler import BaseHandler

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)


class ExecuteActionHandler(BaseHandler):
    def get(self, action):
        allowed_actions = ["start, stop, reset, tare"]
        if (action in allowed_actions):
            Machine.action(action_event=action)
            self.write(json.dumps(
                {"action": action, "status": "ok"}
            ))
        else:
            self.set_status(400)
            self.write(json.dumps(
                {"status": "error",
                 "action": action,
                 "allowed_actions": allowed_actions}
            ))


ACTIONS_HANDLER = [
    (r"/action/(.*)", ExecuteActionHandler),
]
