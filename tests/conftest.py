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
import importlib  # noqa: E402
import types  # noqa: E402


def _stub_if_unavailable(name, build):
    """Install a stub for `name` ONLY if the real module cannot be imported.

    On the machine / Linux CI the real modules (tornado, sounds, ...) exist, and
    a stub must NEVER shadow them (that would break unrelated suites, e.g.
    test_bug_report_api importing tornado.httpclient). On a bare Windows dev box
    these optional/hardware deps are missing, so the stub lets pure modules
    import for unit tests.
    """
    try:
        importlib.import_module(name)
        return  # real module present -> leave it alone
    except Exception:
        pass
    for mod_name, module in build().items():
        sys.modules[mod_name] = module


def _build_pyprctl():
    m = types.ModuleType("pyprctl")
    m.set_name = lambda *a, **k: None
    return {"pyprctl": m}


def _build_pyqrcode():
    m = types.ModuleType("pyqrcode")
    m.create = lambda *a, **k: types.SimpleNamespace(png=lambda *a, **k: None)
    return {"pyqrcode": m}


def _build_tornado_ioloop():
    t = types.ModuleType("tornado")
    io = types.ModuleType("tornado.ioloop")

    class _IOLoop:
        @staticmethod
        def current():
            class _L:
                def add_callback(self, *a, **k):
                    pass

            return _L()

    io.IOLoop = _IOLoop
    t.ioloop = io
    return {"tornado": t, "tornado.ioloop": io}


def _build_sounds():
    s = types.ModuleType("sounds")

    class _SoundPlayer:
        @staticmethod
        def play_event_sound(*a, **k):
            pass

        @staticmethod
        def init(*a, **k):
            pass

    class _Sounds:
        NOTIFICATION = "notification"

    s.SoundPlayer = _SoundPlayer
    s.Sounds = _Sounds
    return {"sounds": s}


_stub_if_unavailable("pyprctl", _build_pyprctl)
_stub_if_unavailable("pyqrcode", _build_pyqrcode)
_stub_if_unavailable("tornado.ioloop", _build_tornado_ioloop)
_stub_if_unavailable("sounds", _build_sounds)
