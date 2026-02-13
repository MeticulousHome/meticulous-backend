from sounds import SoundPlayer

from .base_handler import BaseHandler
from .api import API, APIVersion

from log import MeticulousLogger
from enum import StrEnum

from machine import Machine
import subprocess
import time

logger = MeticulousLogger.getLogger(__name__)


class HardwareTests(StrEnum):
    SPEAKER = "speaker"


class TestsHandler(BaseHandler):
    async def get(self, test_name=None):  # noqa: C901
        if not Machine.is_idle:
            self.write(
                {
                    "error": "testing prohibited",
                    "details": "testing is only allowed in idle machine",
                }
            )
            self.set_status(403)
            self.finish()
            return
        match test_name:
            case HardwareTests.SPEAKER:
                # set the volume to 50%
                try:
                    subprocess.run(
                        ["pactl", "--", "set-sink-volume", "0", "50%"], check=True
                    )
                    time.sleep(0.5)
                except Exception:
                    logger.warning(
                        "could not set the volume to 50%, please cover your ears"
                    )
                finally:
                    if SoundPlayer.play_sound("speaker_test"):
                        self.write({"status": "okay"})
                        self.finish()
                    else:
                        self.set_status(404)
                        self.write(
                            {"error": "sound not found", "details": "speaker_test"}
                        )
                start = time.monotonic()
                while SoundPlayer.is_playing() and time.monotonic() - start < 3.5:
                    time.sleep(0.1)
                logger.debug("speaker test finished")
                try:
                    subprocess.run(
                        ["pactl", "--", "set-sink-volume", "0", "70%"], check=True
                    )
                except Exception:
                    logger.warning("could not set the volume to 70%")


API.register_handler(APIVersion.V1, r"/test[/]*(.*)", TestsHandler)
