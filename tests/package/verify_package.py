"""Check that the built .deb ships everything the backend needs at run time.

Dockerfile.deb stages the package from an explicit COPY list, so a data file
added to the repository is left out silently until the service fails on a
machine (#418: the Python package once shipped without profile_converter/). This compares the extracted
package against the files git tracks and the paths other components rely on:

* the backend binary, and met-config, which meticulous-machine's hawkbit
  setup calls;
* every tracked runtime data file (the small ones are also built into the
  binary, see crates/backend/src/bundled.rs, but the files stay the override);
* the paths the machine image uses from the Python package:
  /opt/meticulous-venv/bin/python3 (its met-config wrapper and smoke report),
  /opt/meticulous-backend/met-config.py and /opt/meticulous-backend/api/web_ui
  (nginx's /debug alias);
* nothing under tests/, and none of the Python backend's sources;
* the binary is split from its debug file: no debug sections, a symbol table
  (function names in crash reports even without the debug file), a GNU build
  id (what Sentry matches the uploaded debug file by) and a .gnu_debuglink.
  With --debug-file, that file belongs to this binary: same build id, the
  CRC the debuglink records, and line tables.

Usage: python3 tests/package/verify_package.py --repo . --package DIR [--debug-file FILE]
where DIR is the output of ``dpkg-deb -x meticulous-backend_*.deb DIR`` and
FILE the meticulous-backend_*.debug built next to the .deb.
"""

import argparse
import fnmatch
import struct
import subprocess
import sys
import zlib
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
# The name the binary's .gnu_debuglink records (gdb and std's backtraces look
# for it next to the binary and under /usr/lib/debug).
DEBUG_LINK_NAME = "meticulous-backend.debug"
# Loaded with the program, so stripping keeps it: rustc's 34-byte pointer to
# gdb's pretty printers, not DWARF.
LOADED_DEBUG_SECTIONS = {".debug_gdb_scripts"}
SHT_NOBITS = 8


def elf_sections(data: bytes) -> dict:
    """Section name to contents (None for a section without contents in the
    file) of a 64-bit little-endian ELF file, what both machines run."""
    if data[:4] != b"\x7fELF" or data[4] != 2 or data[5] != 1:
        raise ValueError("not a 64-bit little-endian ELF file")
    (shoff,) = struct.unpack_from("<Q", data, 0x28)
    shentsize, shnum, shstrndx = struct.unpack_from("<HHH", data, 0x3A)
    headers = []
    for index in range(shnum):
        offset = shoff + index * shentsize
        name, kind = struct.unpack_from("<II", data, offset)
        start, size = struct.unpack_from("<QQ", data, offset + 0x18)
        headers.append((name, kind, start, size))
    _, _, names_start, names_size = headers[shstrndx]
    names = data[names_start : names_start + names_size]
    sections = {}
    for name, kind, start, size in headers:
        label = names[name : names.index(b"\0", name)].decode()
        sections[label] = None if kind == SHT_NOBITS else data[start : start + size]
    return sections


def _align4(n: int) -> int:
    return (n + 3) & ~3


def build_id(sections: dict) -> str | None:
    note = sections.get(".note.gnu.build-id")
    if not note:
        return None
    name_size, desc_size, _ = struct.unpack_from("<III", note, 0)
    start = 12 + _align4(name_size)
    return note[start : start + desc_size].hex()


def debug_link(sections: dict) -> tuple[str, int] | None:
    """The file name and CRC32 a .gnu_debuglink section records."""
    link = sections.get(".gnu_debuglink")
    if not link:
        return None
    name = link.split(b"\0", 1)[0]
    (crc,) = struct.unpack_from("<I", link, _align4(len(name) + 1))
    return name.decode(), crc


def verify_split(binary: Path, debug_file: Path | None) -> list[str]:
    problems = []
    try:
        sections = elf_sections(binary.read_bytes())
    except (ValueError, struct.error) as error:
        return [f"{BINARY} cannot be read as ELF: {error}"]
    debug_sections = sorted(
        name
        for name in sections
        if name.startswith(".debug_") and name not in LOADED_DEBUG_SECTIONS
    )
    if debug_sections:
        problems.append(
            f"{BINARY} still has debug sections ({', '.join(debug_sections)}): "
            "ship the stripped binary and keep the debug file apart"
        )
    if ".symtab" not in sections:
        problems.append(
            f"{BINARY} has no symbol table: crash reports lose their function "
            "names when the debug file is not in Sentry"
        )
    binary_id = build_id(sections)
    if not binary_id:
        problems.append(f"{BINARY} has no GNU build id: Sentry cannot match its debug file")
    link = debug_link(sections)
    if link is None or link[0] != DEBUG_LINK_NAME:
        problems.append(f"{BINARY} has no .gnu_debuglink to {DEBUG_LINK_NAME} (found {link})")
    if debug_file is None:
        return problems

    data = debug_file.read_bytes()
    try:
        debug = elf_sections(data)
    except (ValueError, struct.error) as error:
        return problems + [f"{debug_file.name} cannot be read as ELF: {error}"]
    if build_id(debug) != binary_id:
        problems.append(
            f"{debug_file.name} has build id {build_id(debug)}, the binary {binary_id}"
        )
    if link is not None and zlib.crc32(data) != link[1]:
        problems.append(
            f"{debug_file.name} does not have the CRC the binary's debuglink records"
        )
    for section, purpose in ((".debug_line", "line numbers"), (".symtab", "function names")):
        if not debug.get(section):
            problems.append(f"{debug_file.name} has no {section} ({purpose})")
    return problems


def tracked_files(repo: Path) -> list[str]:
    output = subprocess.run(
        ["git", "ls-files", "--recurse-submodules"],
        cwd=repo,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return [line for line in output.splitlines() if line]


def verify(repo: Path, package: Path, debug_file: Path | None = None) -> list[str]:
    problems = []
    root = package / INSTALL_ROOT
    if not root.is_dir():
        return [f"{INSTALL_ROOT} is not in the package"]

    binary = package / BINARY
    if not binary.is_file():
        problems.append(f"{BINARY} is missing")
    elif not binary.read_bytes()[:4] == b"\x7fELF":
        problems.append(f"{BINARY} is not an executable binary")
    else:
        problems.extend(verify_split(binary, debug_file))

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
    parser.add_argument(
        "--debug-file", type=Path, help="the binary's debug file built next to the .deb"
    )
    args = parser.parse_args()

    problems = verify(
        args.repo.resolve(),
        args.package.resolve(),
        args.debug_file.resolve() if args.debug_file else None,
    )
    for problem in problems:
        print(f"::error::{problem}")
    if problems:
        print(f"{len(problems)} packaging problem(s)", file=sys.stderr)
        sys.exit(1)
    print("package contents match the source tree")


if __name__ == "__main__":
    main()
