"""Boot the backend from this checkout with tests/boot/boot_harness.py.

The harness needs the machine dependency group (pydbus imports PyGObject), so
this test only runs where that group is installed. CI does not depend on it:
the Package workflow boots the installed .deb with the same harness.
"""

import importlib.util
import json
import subprocess
import sys
from pathlib import Path

import pytest

HARNESS = Path(__file__).resolve().parent / "boot" / "boot_harness.py"


@pytest.mark.skipif(
    importlib.util.find_spec("gi") is None,
    reason="needs the machine dependency group (PyGObject); the Package workflow boots the .deb",
)
def test_backend_boots_fresh_and_again_with_its_own_data(tmp_path):
    data_dir = tmp_path / "data"
    for boot in (1, 2):
        report_path = tmp_path / f"boot-{boot}.json"
        result = subprocess.run(
            [
                sys.executable,
                str(HARNESS),
                "--data-dir",
                str(data_dir),
                "--report",
                str(report_path),
                "--boot-index",
                str(boot),
            ],
            capture_output=True,
            text=True,
            timeout=180,
        )
        report = json.loads(report_path.read_text()) if report_path.exists() else {}
        assert (
            result.returncode == 0
        ), f"boot {boot} failed: {report.get('failures')}\n{result.stderr[-4000:]}"
