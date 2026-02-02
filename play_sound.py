import threading
from log import MeticulousLogger
import gi

# Setup GStreamer version before importing
gi.require_version("Gst", "1.0")
# noqa: E402
from gi.repository import Gst  # noqa: E402
from gi.repository import GLib  # noqa: E402

logger = MeticulousLogger.getLogger(__name__)


class PlaysoundException(Exception):
    pass


playsound_lock = threading.Lock()

class SoundPlayer:
    def __init__(self):
        logger.info("Initializing SoundPlayer")
        Gst.init(None)
        self._lock = threading.Lock()
        self._pipeline = None
        self._loop = GLib.MainLoop()
        self._bus = None

    def _on_message(self, bus, message):
        t = message.type
        if t == Gst.MessageType.EOS:
            self._cleanup()
        elif t == Gst.MessageType.ERROR:
            err, debug = message.parse_error()
            logger.error(f"Error: {err}, Debug: {debug}")
            self._cleanup()

    def _cleanup(self):
        if self._pipeline:
            self._pipeline.set_state(Gst.State.NULL)
            self._pipeline = None
        if self._bus:
            self._bus.remove_signal_watch()
            self._bus = None

    def play(self, sound_path):
        with self._lock:
            try:
                # Cleanup any existing playback
                self._cleanup()

                # Create new pipeline
                self._pipeline = Gst.ElementFactory.make("playbin", "player")
                if not self._pipeline:
                    raise PlaysoundException("Could not create pipeline")

                # Setup bus
                self._bus = self._pipeline.get_bus()
                self._bus.add_signal_watch()
                self._bus.connect("message", self._on_message)

                # Set the URI
                if not sound_path.startswith("file://"):
                    sound_path = "file://" + sound_path
                self._pipeline.set_property("uri", sound_path)

                # Start playing
                ret = self._pipeline.set_state(Gst.State.PLAYING)
                if ret == Gst.StateChangeReturn.FAILURE:
                    raise PlaysoundException("Could not play")

                return True

            except Exception as e:
                logger.exception(f"Error playing sound: {e}")
                self._cleanup()
                raise


_player = None


def playsound(sound_path):
    global _player
    if (playsound_lock.acquire(timeout=5)):
        # We dont want multiple glib main loops so ensure it is in the lock
        if _player is None:
            _player = SoundPlayer()

        # This will block until done
        play =_player.play(sound_path)

        playsound_lock.release()
        return play
    else:
        logger.error("Could not acquire playsound lock")
        return False