"""Python's naive local datetimes around DST changes, for crates/history.

insert_history stores datetime.fromtimestamp(time) and search_history reads it
back with datetime.timestamp(); the shot files are named from the same local
time. Writes localtime.json with what CPython answers in a few zones.

    uv run python crates/history/tests/data/generate_localtime.py
"""

import json
import os
import time
from datetime import datetime
from pathlib import Path

ZONES = ["America/New_York", "Europe/Berlin", "Australia/Lord_Howe", "UTC"]
# Instants around the 2026 transitions of the zones above.
TIMESTAMPS = [
    1772953199.0,
    1772953200.0,
    1772953200.5,
    1772956800.0,  # 2026-03-08 New York
    1793512799.999999,
    1793512800.0,
    1793516399.0,
    1793516400.0,  # 2026-11-01 New York
    1774746000.0,
    1774749600.0,  # 2026-03-29 Berlin
    1792890000.0,
    1792893600.0,  # 2026-10-25 Berlin
    1775314800.0,
    1775316600.0,
    1791035400.0,  # Lord Howe (30 minute DST)
    1760000000.25,
    1760000000.0000005,
    1760000000.0000015,
    0.0,
    -1.5,
    1e9 + 0.1234565,
]
WALL_CLOCKS = [
    "2026-03-08 01:59:59.000000",
    "2026-03-08 02:30:00.000000",
    "2026-03-08 03:00:00.000000",
    "2026-11-01 00:59:59.000000",
    "2026-11-01 01:30:00.000000",
    "2026-11-01 02:00:00.000000",
    "2026-03-29 02:30:00.000000",
    "2026-10-25 02:30:00.000000",
    "2026-04-05 01:45:00.000000",
    "2026-10-04 02:15:00.000000",
    "2025-10-09 08:53:20.250000",
    "1970-01-01 00:00:00.000000",
]


def main():
    result = {}
    for zone in ZONES:
        os.environ["TZ"] = zone
        time.tzset()
        result[zone] = {
            "fromtimestamp": [
                [repr(t), datetime.fromtimestamp(t).strftime("%Y-%m-%d %H:%M:%S.%f")]
                for t in TIMESTAMPS
            ],
            "timestamp": [
                [wall, repr(datetime.fromisoformat(wall).timestamp())] for wall in WALL_CLOCKS
            ],
        }
    path = Path(__file__).resolve().parent / "localtime.json"
    path.write_text(json.dumps(result, indent=1) + "\n", encoding="utf-8")
    print(f"wrote {path}")


if __name__ == "__main__":
    main()
