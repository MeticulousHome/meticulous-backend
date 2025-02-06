import json
from log import MeticulousLogger
from .base_handler import BaseHandler
from .api import API, APIVersion
from config import (
    MeticulousConfig,
    CONFIG_SYSTEM,
    SSH_ENABLED
)
from ssh_manager import SSHManager

logger = MeticulousLogger.getLogger(__name__)

class SSHHandler(BaseHandler):
    def get(self):
        """Return current SSH service state"""
        self.write(json.dumps({
            "ssh_enabled": MeticulousConfig[CONFIG_SYSTEM][SSH_ENABLED]
        }))

    def post(self):
        """Update SSH service state"""
        try:
            settings = json.loads(self.request.body)
            
            if "ssh_enabled" not in settings:
                self.set_status(400)
                self.write({
                    "status": "error",
                    "message": "Missing ssh_enabled parameter"
                })
                return
                
            enabled = bool(settings["ssh_enabled"])
            
            if SSHManager.set_ssh_state(enabled):
                MeticulousConfig[CONFIG_SYSTEM][SSH_ENABLED] = enabled
                MeticulousConfig.save()
                self.write({
                    "status": "success",
                    "ssh_enabled": enabled
                })
            else:
                self.set_status(500)
                self.write({
                    "status": "error",
                    "message": "Failed to update SSH service state"
                })
                
        except json.JSONDecodeError:
            self.set_status(400)
            self.write({
                "status": "error",
                "message": "Invalid JSON in request body"
            })
        except Exception as e:
            logger.error(f"Unexpected error in SSH handler: {e}")
            self.set_status(500)
            self.write({
                "status": "error",
                "message": "Internal server error"
            })

# Register the API route
API.register_handler(APIVersion.V1, r"/ssh", SSHHandler)