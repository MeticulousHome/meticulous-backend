from config import MeticulousConfig, CONFIG_SYSTEM, ROOT_PASSWORD
from .base_handler import BaseHandler
from .api import API, APIVersion
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


class LocalAccessHandler(BaseHandler):
    """Base handler that restricts access to local requests only."""

    def prepare(self):
        super().prepare()
        remote_ip = self.request.remote_ip
        request_host = self.request.host.split(":")[0]
        if remote_ip not in ("127.0.0.1", "::1", "localhost") and request_host not in (
            "localhost",
            "127.0.0.1",
        ):
            logger.warning(
                f"Unauthorized access to the password endpoint from {remote_ip}"
            )
            self.set_status(403)
            self.write(
                {
                    "status": "error",
                    "error": "This endpoint can only be accessed locally",
                }
            )
            self.finish()
            return


class RootPasswordHandler(LocalAccessHandler):
    def get(self):
        password = MeticulousConfig[CONFIG_SYSTEM].get(ROOT_PASSWORD)

        if password is None:
            password = "root"

        self.write({"status": "success", "root_password": password})


# Register the handler with the API
API.register_handler(APIVersion.V1, r"/machine/root-password", RootPasswordHandler),
