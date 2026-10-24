"""Check that the built .deb ships everything the backend needs at run time.

Dockerfile.deb stages the package from an explicit COPY list, so a data file
added to the repository is left out silently until the service fails on a
machine (#418: profile_converter/ was missing). This compares the extracted
package against the files git tracks and the paths other components rely on:

* the backend binary, and met-config, which meticulous-machine's hawkbit
  setup calls;
* every tracked runtime data file;
* the paths the machine image uses from the Python package:
  /opt/meticulous-venv/bin/python3 (its met-config wrapper and smoke report),
  /opt/meticulous-backend/met-config.py and /opt/meticulous-backend/api/web_ui
  (nginx's /debug alias);
* nothing under tests/, and none of the Python backend's sources.

Usage: python3 tests/package/verify_package.py --repo . --package DIR
where DIR is the output of ``dpkg-deb -x meticulous-backend_*.deb DIR``.
"""

import argparse
import fnmatch
import subprocess
import sys
from pathlib import Path

INSTALL_ROOT = Path("opt/meticulous-backend")

# Tracked files the backend reads at run time, by pattern, and where they go
# under INSTALL_ROOT (the same relative path).
DATA_FILES = [
    "UI_timezones.json",
    "simple_profile.json",
    "sounds/*",
    "images/default/*.png",
    "images/default/accent_colors.json",
]

BINARY = Path("usr/bin/meticulous-backend")
MET_CONFIG = Path("usr/bin/met-config")
MET_CONFIG_COMMAND = "/usr/bin/meticulous-backend met-config"
REQUIRED_PATHS = {
    "opt/meticulous-backend/met-config.py": "meticulous-machine's met-config wrapper runs it",
    "opt/meticulous-backend/api/web_ui": "nginx serves /debug from it",
}
# Python files the package may carry: the met-config shim only.
ALLOWED_PYTHON = {"met-config.py"}


def tracked_files(repo: Path) -> list[str]:
    output = subprocess.run(
        ["git", "ls-files", "--recurse-submodules"],
        cwd=repo,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return [line for line in output.splitlines() if line]


def verify(repo: Path, package: Path) -> list[str]:
    problems = []
    root = package / INSTALL_ROOT
    if not root.is_dir():
        return [f"{INSTALL_ROOT} is not in the package"]

    binary = package / BINARY
    if not binary.is_file():
        problems.append(f"{BINARY} is missing")
    elif not binary.read_bytes()[:4] == b"\x7fELF":
        problems.append(f"{BINARY} is not an executable binary")

    wrapper = package / MET_CONFIG
    if not wrapper.is_file():
        problems.append(f"{MET_CONFIG} is missing")
    elif MET_CONFIG_COMMAND not in wrapper.read_text():
        problems.append(f"{MET_CONFIG} does not run {MET_CONFIG_COMMAND}")

    venv_python = package / "opt/meticulous-venv/bin/python3"
    if not venv_python.is_symlink():
        problems.append("opt/meticulous-venv/bin/python3 is missing")

    for path, reason in REQUIRED_PATHS.items():
        if not (package / path).exists():
            problems.append(f"{path} is missing ({reason})")

    files = tracked_files(repo)
    for pattern in DATA_FILES:
        matches = [path for path in files if fnmatch.fnmatch(path, pattern)]
        if not matches:
            problems.append(f"no tracked file matches the data pattern {pattern!r}")
        for path in matches:
            if not (root / path).is_file():
                problems.append(f"{path} is runtime data but not packaged")

    for path in sorted(root.rglob("*")):
        relative = path.relative_to(root)
        if not path.is_file():
            continue
        if "tests" in relative.parts:
            problems.append(f"{relative} is test code but shipped")
        elif path.suffix == ".py" and str(relative) not in ALLOWED_PYTHON:
            problems.append(f"{relative} is Python source but shipped")

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
