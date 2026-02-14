from sounds import SoundPlayer

from .base_handler import LocalAccessHandler
from .api import API, APIVersion

from log import MeticulousLogger
from enum import StrEnum

from machine import Machine
import subprocess
import time

from named_thread import NamedThread

logger = MeticulousLogger.getLogger(__name__)


class HardwareTests(StrEnum):
    SPEAKER = "speaker"


class TestsHandler(LocalAccessHandler):
    speaker_test_thread = None
    speaker_test_status = "idle"

    @staticmethod
    def test_speaker(cls):
        # set the volume to 50%
        cls.speaker_test_status = "running"
        try:
            subprocess.run(["pactl", "--", "set-sink-volume", "0", "50%"], check=True)
        except Exception:
            logger.warning("could not set the volume to 50%, please cover your ears")
        finally:
            SoundPlayer.play_sound("speaker_test")
        start = time.monotonic()
        while not SoundPlayer.is_playing():
            if time.monotonic() - start > 1:
                logger.warning("timeout starting to play test audio")
                return

        logger.debug("Playing audio stream")
        while SoundPlayer.is_playing() or time.monotonic() - start < 1:
            time.sleep(0.1)
        logger.debug("speaker test finished")
        try:
            subprocess.run(["pactl", "--", "set-sink-volume", "0", "70%"], check=True)
        except Exception:
            logger.warning("could not set the volume to 70%")

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
                try:
                    self.speaker_test_thread = NamedThread(
                        name="speaker test",
                        target=TestsHandler.test_speaker,
                        args=(self,),
                    )
                    self.speaker_test_thread.start()
                    self.write({"status": "okay"})
                    self.finish()
                except Exception as e:
                    self.write(
                        {
                            "error": "issue encountered testing speaker",
                            "details": f"{e}",
                        }
                    )
                    self.set_status(400)


API.register_handler(APIVersion.V1, r"/test[/]*(.*)", TestsHandler)
