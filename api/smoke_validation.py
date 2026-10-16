from .api import API, APIVersion
from .base_handler import LocalAccessHandler
from log import MeticulousLogger
from smoke_validation import SmokeValidationManager

logger = MeticulousLogger.getLogger(__name__)


class SmokeValidationHandler(LocalAccessHandler):
    async def get(self):
        from machine import Machine

        try:
            self.write(SmokeValidationManager.get_status(Machine))
        except Exception as exc:
            logger.warning("Failed to build smoke validation status", exc_info=exc)
            self.set_status(500)
            self.write({"status": "error", "error": str(exc)})


API.register_handler(APIVersion.V1, r"/smoke-validation", SmokeValidationHandler)
