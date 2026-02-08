import os
import sys

# Set environment variables before any application modules are imported.
# This ensures config.py, log.py, etc. pick up test-friendly paths.
os.environ.setdefault("CONFIG_PATH", "/tmp/meticulous-test/config")
os.environ.setdefault("LOG_PATH", "/tmp/meticulous-test/logs")
os.environ.setdefault("HISTORY_PATH", "/tmp/meticulous-test/history")
os.environ.setdefault("DEBUG_HISTORY_PATH", "/tmp/meticulous-test/history/debug")

# Add the backend root to sys.path so imports like "from config import ..."
# work without installing the package.
backend_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if backend_root not in sys.path:
    sys.path.insert(0, backend_root)
