import os
import sys
from types import ModuleType

# Set environment variables before any application modules are imported.
# This ensures config.py, log.py, etc. pick up test-friendly paths.
os.environ.setdefault("CONFIG_PATH", "/tmp/meticulous-test/config")
os.environ.setdefault("LOG_PATH", "/tmp/meticulous-test/logs")
os.environ.setdefault("HISTORY_PATH", "/tmp/meticulous-test/history")
os.environ.setdefault("DEBUG_HISTORY_PATH", "/tmp/meticulous-test/history/debug")
os.environ.setdefault("ALARMS_PATH", "/tmp/meticulous-test/alarms")
os.environ.setdefault(
    "SMOKE_VALIDATION_STATE_FILE", "/tmp/meticulous-test/smoke-validation.json"
)
os.environ.setdefault("USER_SOUNDS", "/tmp/meticulous-test/sounds")
# The real redaction key lives in /root, which the test runner cannot read.
# Without this every record would come out as the failure placeholder.
os.makedirs("/tmp/meticulous-test", exist_ok=True)
os.environ.setdefault("REDACTION_KEY_PATH", "/tmp/meticulous-test/.redaction_key")

# machine.py imports the machine-only gpiod package through its UART and sound adapters.
# Tests replace those adapters and must remain runnable with only the dev dependency group.
gpiod = ModuleType("gpiod")


class StubLineRequest:
    DIRECTION_OUTPUT = 1


gpiod.line_request = StubLineRequest
sys.modules.setdefault("gpiod", gpiod)

# pydbus needs PyGObject, which is in the machine dependency group. api/settings.py
# reaches it at import time through ssh_manager -> system_services. Where it is not
# installed, a stand-in lets those modules import; talking to the bus still fails.
try:
    import pydbus  # noqa: F401
except ImportError:
    pydbus = ModuleType("pydbus")

    def _no_system_bus(*args, **kwargs):
        raise RuntimeError("no D-Bus system bus in the unit tests")

    pydbus.SystemBus = _no_system_bus
    pydbus.SessionBus = _no_system_bus
    sys.modules["pydbus"] = pydbus

# back.py and machine.py build Sentry clients with the production DSNs written in.
# Events captured by a test (ESP32 log lines, diagnostics) would be sent to those
# projects, so every client the tests create has its DSN dropped and sends nothing.
# Must run before machine.py is imported. tests/test_sentry_offline.py checks it.
import sentry_sdk  # noqa: E402

_production_client = sentry_sdk.Client
_production_init = sentry_sdk.init


class _OfflineSentryClient(_production_client):
    def __init__(self, *args, **kwargs):
        kwargs["dsn"] = None
        super().__init__(*args[1:], **kwargs)  # the DSN is the only positional


def _offline_sentry_init(*args, **kwargs):
    kwargs["dsn"] = None
    return _production_init(*args[1:], **kwargs)


sentry_sdk.Client = _OfflineSentryClient
sentry_sdk.init = _offline_sentry_init

# Add the backend root to sys.path so imports like "from config import ..."
# work without installing the package.
backend_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if backend_root not in sys.path:
    sys.path.insert(0, backend_root)
