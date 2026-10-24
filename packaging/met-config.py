#!/usr/bin/env python3
"""met-config for scripts that still call it through Python.

meticulous-machine's /usr/bin/met-config runs
``/opt/meticulous-venv/bin/python3 /opt/meticulous-backend/met-config.py``;
the backend binary implements the command now.
"""

import os
import sys

os.execv("/usr/bin/meticulous-backend", ["meticulous-backend", "met-config", *sys.argv[1:]])
