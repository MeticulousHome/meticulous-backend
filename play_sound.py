import threading
from log import MeticulousLogger
import gi

# Setup GStreamer version before importing
gi.require_version("Gst", "1.0")
# noqa: E402
from gi.repository import Gst  # noqa: E402

logger = MeticulousLogger.getLogger(__name__)

# Global GStreamer initialization - must happen exactly once
_gst_initialized = False
_gst_init_lock = threading.Lock()


def _ensure_gst_init():
    """Initialize GStreamer exactly once, thread-safely."""
    global _gst_initialized
    if not _gst_initialized:
        with _gst_init_lock:
            if not _gst_initialized:
                Gst.init(None)
                _gst_initialized = True


class PlaysoundException(Exception):
    pass


class SoundPlayer:
    def __init__(self):
        logger.info("Initializing SoundPlayer")
        # Note: Gst.init() is called via _ensure_gst_init() before instantiation
        self._lock = threading.Lock()
        self._pipeline = None
        self._bus = None

    def _on_message(self, bus, message):
        """Callback from GStreamer thread for non-blocking playback."""
        t = message.type
        if t == Gst.MessageType.EOS:
            with self._lock:
                self._cleanup_internal()
        elif t == Gst.MessageType.ERROR:
            err, debug = message.parse_error()
            logger.error(f"Error: {err}, Debug: {debug}")
            with self._lock:
                self._cleanup_internal()

    def _cleanup_internal(self):
        """Internal cleanup - caller must hold self._lock."""
        if self._pipeline:
            self._pipeline.set_state(Gst.State.NULL)
            self._pipeline = None
        if self._bus:
            self._bus.remove_signal_watch()
            self._bus = None

    def play(self, sound_path, block=True):
        # Setup pipeline while holding lock
        with self._lock:
            try:
                # Cleanup any existing playback
                self._cleanup_internal()

                # Create new pipeline
                self._pipeline = Gst.ElementFactory.make("playbin", "player")
                if not self._pipeline:
                    raise PlaysoundException("Could not create pipeline")

                # Setup bus
                self._bus = self._pipeline.get_bus()

                # Only use signal watch for non-blocking mode
                # For blocking mode, we use timed_pop_filtered which consumes the message
                if not block:
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

                if not block:
                    # Non-blocking: return immediately, _on_message handles cleanup
                    return True

                # For blocking mode, save bus reference before releasing lock
                bus = self._bus

            except Exception as e:
                logger.exception(f"Error playing sound: {e}")
                self._cleanup_internal()
                raise

        # Blocking mode: wait for completion WITHOUT holding lock
        # This prevents deadlock if GStreamer thread needs the lock
        try:
            bus.timed_pop_filtered(
                Gst.CLOCK_TIME_NONE, Gst.MessageType.ERROR | Gst.MessageType.EOS
            )
        finally:
            # Reacquire lock for cleanup
            with self._lock:
                self._cleanup_internal()

        return True


_player = None
_player_lock = threading.Lock()


def playsound(sound_path, block=True):
    global _player
    _ensure_gst_init()
    if _player is None:
        with _player_lock:
            if _player is None:
                _player = SoundPlayer()
    return _player.play(sound_path, block)
