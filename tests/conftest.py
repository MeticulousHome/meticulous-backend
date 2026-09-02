import os
import sys

# Set environment variables before any application modules are imported.
# This ensures config.py, log.py, etc. pick up test-friendly paths.
os.environ.setdefault("CONFIG_PATH", "/tmp/meticulous-test/config")
os.environ.setdefault("LOG_PATH", "/tmp/meticulous-test/logs")
os.environ.setdefault("HISTORY_PATH", "/tmp/meticulous-test/history")
os.environ.setdefault("DEBUG_HISTORY_PATH", "/tmp/meticulous-test/history/debug")
# The real redaction key lives in /root, which the test runner cannot read.
# Without this every record would come out as the failure placeholder.
os.makedirs("/tmp/meticulous-test", exist_ok=True)
os.environ.setdefault("REDACTION_KEY_PATH", "/tmp/meticulous-test/.redaction_key")
os.environ.setdefault("IDENTITY_PATH", "/tmp/meticulous-test/identity/")

# Add the backend root to sys.path so imports like "from config import ..."
# work without installing the package.
backend_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if backend_root not in sys.path:
    sys.path.insert(0, backend_root)

# notifications.py pulls in SoundPlayer, which imports hardware/optional deps
# (playsound3, gpiod, ...) absent from the CI/test environment. Stub the
# `sounds` module with just what notifications.py uses so importing it during
# collection does not fail.
import types  # noqa: E402

# Leaf system/optional deps absent from the CI/test environment. Stub them so
# importing notifications.py (via named_thread / pyqrcode) does not fail.
for _name in ("pyprctl",):
    if _name not in sys.modules:
        _m = types.ModuleType(_name)
        _m.set_name = lambda *a, **k: None
        sys.modules[_name] = _m

if "pyqrcode" not in sys.modules:
    _qr = types.ModuleType("pyqrcode")
    _qr.create = lambda *a, **k: types.SimpleNamespace(png=lambda *a, **k: None)
    sys.modules["pyqrcode"] = _qr

# tornado is not installed in the CI/test venv; stub tornado.ioloop so modules
# that import it (socket_registry) load. Unit tests never drive the loop.
if "tornado" not in sys.modules:
    _t = types.ModuleType("tornado")
    _io = types.ModuleType("tornado.ioloop")

    class _IOLoop:
        @staticmethod
        def current():
            class _L:
                def add_callback(self, *a, **k):
                    pass
            return _L()

    _io.IOLoop = _IOLoop
    _t.ioloop = _io
    sys.modules["tornado"] = _t
    sys.modules["tornado.ioloop"] = _io

if "sounds" not in sys.modules:
    _sounds = types.ModuleType("sounds")

    class _SoundPlayer:
        @staticmethod
        def play_event_sound(*a, **k):
            pass

        @staticmethod
        def init(*a, **k):
            pass

    class _Sounds:
        NOTIFICATION = "notification"

    _sounds.SoundPlayer = _SoundPlayer
    _sounds.Sounds = _Sounds
    sys.modules["sounds"] = _sounds
