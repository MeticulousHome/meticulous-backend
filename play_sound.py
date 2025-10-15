import threading

import gi

from log import MeticulousLogger

# Setup GStreamer version before importing
gi.require_version("Gst", "1.0")
from gi.repository import GLib, Gst  # noqa: E402

logger = MeticulousLogger.getLogger(__name__)


class PlaysoundException(Exception):
    pass


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

    def play(self, sound_path, block=True):
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

                if block:
                    # Wait for EOS or ERROR
                    self._bus.timed_pop_filtered(
                        Gst.CLOCK_TIME_NONE, Gst.MessageType.ERROR | Gst.MessageType.EOS
                    )
                    self._cleanup()

                return True

            except Exception as e:
                logger.exception(f"Error playing sound: {e}")
                self._cleanup()
                raise


_player = None


def playsound(sound_path, block=True):
    global _player
    if _player is None:
        _player = SoundPlayer()
    return _player.play(sound_path, block)
