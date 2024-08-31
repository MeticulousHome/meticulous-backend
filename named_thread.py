import pyprctl
import threading

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


class NamedThread(threading.Thread):
    def __init__(self, name: str, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.name: str = name

    def run(self):
        try:
            pyprctl.set_name(self.name)
            logger.debug(f"Thread {self.name} started")
            super().run()
        except Exception:
            # FIXME we should handle crashes here
            pass
