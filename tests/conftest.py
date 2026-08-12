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
os.environ.setdefault("MOTOR_ENERGY_PATH", "/tmp/meticulous-test/energy")
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

# Add the backend root to sys.path so imports like "from config import ..."
# work without installing the package.
backend_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if backend_root not in sys.path:
    sys.path.insert(0, backend_root)
