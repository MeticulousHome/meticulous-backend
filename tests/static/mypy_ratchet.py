"""Fail on type errors mypy finds that tests/static/mypy-baseline.txt does not list.

The backend has hundreds of errors mypy reports today, so mypy cannot gate
merges directly. The baseline records them; a change that adds an error fails,
and one that removes errors must shrink the baseline, so the count only goes
down. Errors are keyed by file, error code and message, without line numbers,
so moving code does not churn the baseline.

    uv run --with mypy==1.18.2 python tests/static/mypy_ratchet.py           # check
    uv run --with mypy==1.18.2 python tests/static/mypy_ratchet.py --update  # rewrite

The mypy version is pinned in CI because a new release changes what it reports.
"""

import argparse
import collections
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
BASELINE = Path(__file__).resolve().parent / "mypy-baseline.txt"
MYPY_ARGS = [
    "--ignore-missing-imports",
    "--check-untyped-defs",
    "--explicit-package-bases",
    "--no-error-summary",
    "--hide-error-context",
    "--no-color-output",
    "--show-error-codes",
    "--exclude",
    r"^(tests|log_redactor|alembic|\.venv)/",
    ".",
]
ERROR = re.compile(r"^(?P<file>[^:]+):\d+: error: (?P<message>.*?)\s+\[(?P<code>[a-z-]+)\]$")


def current_errors() -> collections.Counter:
    result = subprocess.run(
        [sys.executable, "-m", "mypy", *MYPY_ARGS],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
    )
    if result.returncode not in (0, 1):  # 2 means mypy itself failed
        sys.exit(f"mypy failed to run:\n{result.stdout}{result.stderr}")
    errors = collections.Counter()
    for line in result.stdout.splitlines():
        match = ERROR.match(line)
        if match:
            errors[f"{match['file']}: [{match['code']}] {match['message']}"] += 1
    return errors, result.stdout


def read_baseline() -> collections.Counter:
    baseline = collections.Counter()
    for line in BASELINE.read_text().splitlines():
        if line and not line.startswith("#"):
            count, _, key = line.partition(" ")
            baseline[key] = int(count)
    return baseline


def write_baseline(errors: collections.Counter):
    lines = [
        "# Type errors mypy reports on gates/main, by file, code and message, with",
        "# their count. Regenerate with tests/static/mypy_ratchet.py --update.",
    ]
    lines += [f"{count} {key}" for key, count in sorted(errors.items())]
    BASELINE.write_text("\n".join(lines) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--update", action="store_true", help="rewrite the baseline")
    args = parser.parse_args()

    errors, output = current_errors()
    if args.update:
        write_baseline(errors)
        print(f"baseline written: {sum(errors.values())} errors")
        return

    baseline = read_baseline()
    new = errors - baseline
    fixed = baseline - errors
    if new:
        print("New type errors (not in tests/static/mypy-baseline.txt):")
        for key in sorted(new):
            matching = [line for line in output.splitlines() if _key_of(line) == key]
            for line in matching:
                print(f"::error::{line}")
    if fixed:
        print(
            f"{sum(fixed.values())} baseline error(s) no longer reported. Shrink the "
            "baseline: tests/static/mypy_ratchet.py --update"
        )
        for key in sorted(fixed):
            print(f"  fixed: {key}")
    if new or fixed:
        sys.exit(1)
    print(f"no new type errors ({sum(errors.values())} in the baseline)")


def _key_of(line: str) -> str | None:
    match = ERROR.match(line)
    return f"{match['file']}: [{match['code']}] {match['message']}" if match else None


if __name__ == "__main__":
    main()
