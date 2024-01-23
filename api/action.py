import json
from machine import Machine
from .base_handler import BaseHandler

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)


class ExecuteActionHandler(BaseHandler):
    def get(self, action):
        Machine.action(action_event=action)
        self.write(json.dumps(
            {"action": action, "status": "send"}
        ))

ACTIONS_HANDLER = [
    (r"/action/(.*)", ExecuteActionHandler),
]
