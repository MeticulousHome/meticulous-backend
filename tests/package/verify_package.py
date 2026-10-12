"""Check that the built .deb ships every file the backend needs at run time.

Dockerfile.deb stages the package from an explicit COPY list, so a module or
data file added to the repository is left out silently until the service fails
to start on a machine (#418: profile_converter/ was missing). This compares the
extracted package against the files git tracks:

* every tracked Python file must be in the package, unless NOT_PACKAGED says
  why it stays out;
* every tracked data file under a runtime data directory must be there too;
* nothing under tests/ may be shipped;
* the launcher must point at the packaged interpreter and entry point.

Usage: python3 tests/package/verify_package.py --repo . --package DIR
where DIR is the output of ``dpkg-deb -x meticulous-backend_*.deb DIR``.
"""

import argparse
import fnmatch
import subprocess
import sys
from pathlib import Path

INSTALL_ROOT = Path("opt/meticulous-backend")

# Tracked Python files that are deliberately not in the package.
NOT_PACKAGED = {
    "tests/*": "test suite",
    "*/tests/*": "a submodule's own test suite",
    ".github/*": "CI scripts",
    "log_redactor/*/*": "only log_redactor/*.py is the runtime module",
    "profile_schema/*": "JSON schema repository; only its data files ship",
    "images/default/*": "image repository; only its images ship",
}

# Tracked non-Python files the backend reads at run time.
DATA_FILES = [
    "alembic.ini",
    "alembic/env.py",
    "alembic/script.py.mako",
    "UI_timezones.json",
    "simple_profile.json",
    "esp_serial/connection/*.json",
    "profile_schema/schema.json",
    "pour_over_profile_schema/*.json",
    "sounds/*",
    "images/default/*.png",
    "images/default/accent_colors.json",
]

LAUNCHER = Path("usr/bin/meticulous-backend")
LAUNCHER_COMMAND = "/opt/meticulous-venv/bin/python3 /opt/meticulous-backend/back.py"


def tracked_files(repo: Path) -> list[str]:
    output = subprocess.run(
        ["git", "ls-files", "--recurse-submodules"],
        cwd=repo,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return [line for line in output.splitlines() if line]


def excluded_reason(path: str) -> str | None:
    for pattern, reason in NOT_PACKAGED.items():
        if fnmatch.fnmatch(path, pattern):
            return reason
    return None


def verify(repo: Path, package: Path) -> list[str]:
    problems = []
    root = package / INSTALL_ROOT
    if not root.is_dir():
        return [f"{INSTALL_ROOT} is not in the package"]

    files = tracked_files(repo)

    for path in files:
        if not path.endswith(".py") or excluded_reason(path):
            continue
        if not (root / path).is_file():
            problems.append(f"{path} is tracked but not packaged")

    for pattern in DATA_FILES:
        matches = [path for path in files if fnmatch.fnmatch(path, pattern)]
        if not matches:
            problems.append(f"no tracked file matches the data pattern {pattern!r}")
        for path in matches:
            if not (root / path).is_file():
                problems.append(f"{path} is runtime data but not packaged")

    shipped_tests = sorted(
        str(path.relative_to(root))
        for path in root.rglob("*")
        if path.is_file() and ("tests" in path.relative_to(root).parts)
    )
    for path in shipped_tests:
        problems.append(f"{path} is test code but shipped")

    launcher = package / LAUNCHER
    if not launcher.is_file():
        problems.append(f"{LAUNCHER} is missing")
    elif LAUNCHER_COMMAND not in launcher.read_text():
        problems.append(f"{LAUNCHER} does not run {LAUNCHER_COMMAND}")

    venv_python = package / "opt/meticulous-venv/bin/python3"
    if not (venv_python.exists() or venv_python.is_symlink()):
        problems.append("opt/meticulous-venv/bin/python3 is missing")

    return problems


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--package", type=Path, required=True)
    args = parser.parse_args()

    problems = verify(args.repo.resolve(), args.package.resolve())
    for problem in problems:
        print(f"::error::{problem}")
    if problems:
        print(f"{len(problems)} packaging problem(s)", file=sys.stderr)
        sys.exit(1)
    print("package contents match the source tree")


if __name__ == "__main__":
    main()
