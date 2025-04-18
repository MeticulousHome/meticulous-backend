from config import MeticulousConfig, CONFIG_SYSTEM, ROOT_PASSWORD
from .base_handler import LocalAccessHandler
from .api import API, APIVersion
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


class RootPasswordHandler(LocalAccessHandler):
    def get(self):
        password = MeticulousConfig[CONFIG_SYSTEM].get(ROOT_PASSWORD)

        if password is None:
            password = "root"

        self.write({"status": "success", "root_password": password})


# Register the handler with the API
API.register_handler(APIVersion.V1, r"/machine/root-password", RootPasswordHandler),
