"""Boot the real backend binary with only the machine's hardware replaced.

Runs ``meticulous-backend`` as the systemd service does, in production mode
(``BACKEND=FIKA``), and checks from outside, as the Dial and the app see it,
that the backend came up:

* the HTTP server answers and every read-only GET route answers below 500,
* a Socket.IO client receives the ``status`` stream with its documented keys,
* each event a client sends does what the backend's handler says: whitelisted
  actions reach the ESP32 and the rest do not, calibrate writes its command,
  profileHover is relayed to the other clients, and a notification is
  delivered and acknowledged,
* the ESP32 was greeted over the UART (``\\x03`` then ``action,info``),
* the history database was migrated to the required alembic revision,
* nothing logged at ERROR or above while booting.

What is replaced is the hardware and the host OS the machine image provides,
each at its boundary: the ESP32 UART is a pty this harness holds (it records
what the backend writes and only speaks when a check needs the ESP to), GPIO,
I2C, BlueZ, systemd, the backlight, audio and disk imaging are left alone
(``METICULOUS_FAKE_HARDWARE=1``), and the host commands the image provides are
recording stubs on PATH. Everything else runs for real against throwaway
directories, including the D-Bus system bus, which the caller provides
(``dbus-daemon --system``).

It writes a JSON report to ``--report`` and exits 0 only when every check
passed. Booting twice with the same --data-dir checks the restart of a machine
that already has its own config, profiles and history.

For integration tests against a real firmware: --esp-uart connects the backend
to a serial device instead of the harness's pty (fika-sil's --pty-link), and
--serve skips the checks, writes READY_FILE once the server answers and keeps
the backend running until SIGTERM. The report then holds the ERROR records, the
UART greeting and the database check of the whole run.

Usage: python tests/boot/boot_harness.py --data-dir DIR --report FILE
           [--binary PATH] [--package-root DIR] [--esp-uart DEVICE]
           [--serve READY_FILE]
"""

import argparse
import asyncio
import json
import os
import re
import signal
import socket
import sqlite3
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO_ROOT = HERE.parent.parent

# Seconds the whole boot plus checks may take before the watchdog fails it.
BOOT_TIMEOUT_SECONDS = float(os.getenv("BOOT_TIMEOUT_SECONDS", "90"))

# The alembic revision the backend migrates the history database to.
DB_VERSION_REQUIRED = "c4d2e9f1a7b3"

# Keys every ``status`` event carries. The Dial and the mobile app read these
# by name.
STATUS_EVENT_KEYS = {
    "name",
    "sensors",
    "time",
    "profile",
    "profile_time",
    "state",
    "extracting",
    "setpoints",
    "loaded_profile",
    "id",
}

# GET routes the route sweep leaves out, each with the reason. Anything not
# here and without a capture group is requested.
SWEEP_SKIP = {
    # Starts the ESP32's scale master calibration: an actuation, not a read.
    "/api/v1/scaleCalibrate": "actuates the scale",
}

# ERROR records that booting outside a machine image produces by design.
# Matched against the start of the message. Anything else at ERROR fails.
EXPECTED_BOOT_ERRORS = {
    # /opt/ROOTFS_BUILD_DATE and friends are written by meticulous-machine.
    "Could not get build channel": "image metadata files are absent",
    "Could not get build timestamp": "image metadata files are absent",
}

# Backend bugs the sweep found and that are not fixed yet. Each route's 500 and
# the ERROR records starting with the listed messages are reported but do not
# fail the boot. A listed route that stops answering 500 does fail it, so the
# entry is removed together with the fix.
KNOWN_ISSUES = {}
_KNOWN_ERROR_PREFIXES = [prefix for issue in KNOWN_ISSUES.values() for prefix in issue["errors"]]

# `<date> <time>,<ms> <logger> <LEVEL> <message>`, the backend's log lines.
LOG_LINE = re.compile(r"^\d{4}-\d\d-\d\d \d\d:\d\d:\d\d,\d{3} (\S+) (DEBUG|INFO|WARNING|ERROR|CRITICAL) (.*)$")

# Host commands the machine image provides, replaced by recording stubs on
# PATH. Keyed by the first argument, each prints what the real tool prints for
# the call the backend makes at boot; any other call prints nothing and
# succeeds. Commands that change the host (reboot, mount, hostnamectl, ...)
# are stubbed so that a boot that calls one records it instead of acting on
# the CI runner. host_commands in the report lists every call.
HOST_COMMANDS = {
    "nmcli": {},
    "pactl": {},
    "gst-play-1.0": {},
    "i2cget": {"-f": "0x21"},  # SOM revision register
    "timedatectl": {
        "status": "                Time zone: Etc/UTC (UTC, +0000)",
        "list-timezones": "Etc/UTC\nAmerica/Mexico_City\nEurope/Berlin",
    },
    "chpasswd": {},
    "systemctl": {},
    "hostnamectl": {},
    "reboot": {},
    "mount": {},
    "umount": {},
    "partprobe": {},
    "rauc-hawkbit-updater": {},
}


def _free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def _environment(data_dir: Path, code_root: Path, port: int) -> dict:
    """Point every path the backend writes to into ``data_dir``."""
    user = data_dir / "meticulous-user"
    paths = {
        "CONFIG_PATH": user / "config",
        "LOG_PATH": user / "logs",
        "HISTORY_PATH": user / "history",
        "DEBUG_HISTORY_PATH": user / "history" / "debug",
        "MOTOR_ENERGY_PATH": user / "syslog" / "energy",
        "ALARMS_PATH": user / "syslog" / "alarms",
        "PROFILE_PATH": user / "profiles",
        "POUR_OVER_PROFILE_PATH": user / "pour-over-profiles",
        "IMAGES_PATH": user / "profile-images",
        "USER_SOUNDS": user / "sounds",
        "REPORTS_DIR": user / "reports",
        "USER_DB_MIGRATION_DIR": user / ".dbmigrations",
        "DEVICE_UUID_CACHE_PATH": user / ".device-identity" / "device-uuid",
        "SMOKE_VALIDATION_STATE_FILE": user / "smoke-validation.json",
        "SMOKE_VALIDATION_VERSION_FILE": data_dir / "opt" / "image-build-version",
        "REDACTION_KEY_PATH": data_dir / "root" / ".redaction_key",
        "TIMEZONE_JSON_FILE_PATH": data_dir / "usr" / "share" / "zoneinfo" / "UI_timezones.json",
        "UPDATE_PATH": data_dir / "opt" / "meticulous-firmware",
        "ISSUE_PATH": data_dir / "etc-issue",
        "METICULOUS_PACKAGE_ROOT": code_root,
        "DEFAULT_IMAGES": code_root / "images" / "default",
        "SYSTEM_SOUNDS": code_root / "sounds",
        "SIMPLE_PROFILE": code_root / "simple_profile.json",
    }
    env = dict(os.environ)
    for key, value in paths.items():
        env[key] = str(value)
    for key in ("REDACTION_KEY_PATH", "TIMEZONE_JSON_FILE_PATH", "DEVICE_UUID_CACHE_PATH"):
        Path(env[key]).parent.mkdir(parents=True, exist_ok=True)
    env["BACKEND"] = "FIKA"
    env["PORT"] = str(port)
    env["METICULOUS_FAKE_HARDWARE"] = "1"
    env.pop("SENTRY", None)
    env.pop("DEBUG", None)
    env.pop("LOGLEVEL", None)
    return env


def _install_host_commands(data_dir: Path, boot_index: int, env: dict) -> Path:
    bin_dir = data_dir / "host-bin"
    bin_dir.mkdir(parents=True, exist_ok=True)
    log = data_dir / f"host-commands-{boot_index}.log"
    for name, outputs in HOST_COMMANDS.items():
        cases = "".join(
            f"    {argument}) printf '%s\\n' '{output}' ;;\n" for argument, output in outputs.items()
        )
        stub = bin_dir / name
        stub.write_text(
            "#!/bin/sh\n"
            f'echo "{name} $*" >> "{log}"\n'
            f'case "$1" in\n{cases}    *) ;;\nesac\n'
            "exit 0\n"
        )
        stub.chmod(0o755)
    env["PATH"] = f"{bin_dir}{os.pathsep}{env.get('PATH', '')}"
    return log


class FakeEsp:
    """The ESP32's UART: a pty whose other end this harness holds. Records what
    the backend writes and only answers when a check writes a line."""

    def __init__(self):
        self.master, slave = os.openpty()
        self.device = os.ttyname(slave)
        self._slave = slave
        self.written = bytearray()
        self._lock = threading.Lock()
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        while True:
            try:
                data = os.read(self.master, 4096)
            except OSError:
                return
            if not data:
                return
            with self._lock:
                self.written += data

    def snapshot(self) -> bytes:
        with self._lock:
            return bytes(self.written)

    def send_line(self, line: str):
        os.write(self.master, (line + "\r\n").encode())


class Backend:
    """The binary under test, its log captured line by line."""

    def __init__(self, binary: Path, env: dict, log_path: Path):
        self.log_path = log_path
        self.lines = []
        self._log = log_path.open("w")
        self.process = subprocess.Popen(
            [str(binary)],
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            errors="replace",
        )
        threading.Thread(target=self._pump, daemon=True).start()

    def _pump(self):
        for line in self.process.stdout:
            self.lines.append(line.rstrip("\n"))
            self._log.write(line)
            self._log.flush()

    def errors(self) -> list:
        records = []
        for line in self.lines:
            match = LOG_LINE.match(line)
            if not match or match.group(2) not in ("ERROR", "CRITICAL"):
                continue
            message = match.group(3)
            if any(message.startswith(prefix) for prefix in EXPECTED_BOOT_ERRORS):
                continue
            if any(message.startswith(prefix) for prefix in _KNOWN_ERROR_PREFIXES):
                continue
            records.append(f"{match.group(1)}: {message}")
        return records

    def stop(self):
        if self.process.poll() is None:
            self.process.send_signal(signal.SIGTERM)
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()


def _http_get(url: str, timeout: float = 20):
    try:
        with urllib.request.urlopen(url, timeout=timeout) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


def _wait_until_serving(base: str, backend: Backend, report: dict) -> bool:
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if backend.process.poll() is not None:
            report["failures"].append(f"the backend exited with {backend.process.returncode}")
            return False
        try:
            _http_get(f"{base}/api/v1/settings", timeout=2)
            return True
        except (ConnectionError, OSError, urllib.error.URLError):
            time.sleep(0.2)
    report["failures"].append("HTTP server never accepted a connection")
    return False


def _contract_routes(binary: Path) -> list:
    output = subprocess.run([str(binary), "contract"], capture_output=True, text=True, check=True).stdout
    return json.loads(output)["http_routes.json"]


def _http_checks(base: str, binary: Path, report: dict):
    swept = {}
    seen = set()
    for route in _contract_routes(binary):
        path = route["path"]
        if "(" in path or path in SWEEP_SKIP or "get" not in route["methods"] or path in seen:
            continue
        seen.add(path)
        path = path.replace("[/]*", "")
        try:
            code, _ = _http_get(f"{base}{path}")
        except Exception as error:  # timeouts, resets
            code = f"{type(error).__name__}: {error}"
        swept[path] = code
        failed = not isinstance(code, int) or code >= 500
        if path in KNOWN_ISSUES:
            if not failed:
                report["failures"].append(
                    f"GET {path} answered {code}: the known issue is fixed, remove its KNOWN_ISSUES entry"
                )
        elif failed:
            report["failures"].append(f"GET {path} answered {code}")
    report["routes"] = swept
    report["known_issues"] = {path: issue["bug"] for path, issue in KNOWN_ISSUES.items() if path in swept}
    for path in KNOWN_ISSUES.keys() - swept.keys():
        report["failures"].append(f"KNOWN_ISSUES lists {path}, which is no longer a GET route")


async def _wait_for(condition, timeout: float) -> bool:
    deadline = time.monotonic() + timeout
    while not condition():
        if time.monotonic() > deadline:
            return False
        await asyncio.sleep(0.05)
    return True


async def _socketio_checks(base: str, report: dict):
    import socketio

    received = {}
    sio = socketio.AsyncClient(reconnection=False)

    @sio.on("*")
    async def any_event(event, data=None):
        received.setdefault(event, data)

    try:
        await sio.connect(base, transports=["websocket"], wait_timeout=10)
    except Exception as error:
        report["failures"].append(f"Socket.IO connect failed: {error}")
        return
    await _wait_for(lambda: "status" in received, 10)
    await sio.disconnect()

    report["socketio_events"] = sorted(received)
    status = received.get("status")
    if not isinstance(status, dict):
        report["failures"].append("no status event within 10 s of connecting")
        return
    missing = STATUS_EVENT_KEYS - status.keys()
    unexpected = status.keys() - STATUS_EVENT_KEYS
    if missing:
        report["failures"].append(f"status event lacks {sorted(missing)}")
    if unexpected:
        report["failures"].append(f"status event has undocumented keys {sorted(unexpected)}")


async def _socketio_inbound_checks(base: str, esp: FakeEsp, report: dict):
    """What the server does with each event a client sends."""
    import socketio

    failures = report["failures"]
    received = {"dial": [], "app": []}
    clients = {}
    for name in received:
        client = socketio.AsyncClient(reconnection=False)

        def recorder(name):
            async def record(event, data=None):
                received[name].append((event, data))

            return record

        client.on("*", recorder(name))
        await client.connect(base, transports=["websocket"], wait_timeout=10)
        clients[name] = client
    dial, app = clients["dial"], clients["app"]

    def written_since(mark: int) -> bytes:
        return esp.snapshot()[mark:]

    # A whitelisted ESP32 action is forwarded as action,<name>.
    mark = len(esp.snapshot())
    await dial.emit("action", "tare")
    if not await _wait_for(lambda: b"action,tare\x03" in written_since(mark), 3):
        failures.append("socket.io action 'tare' was not forwarded to the ESP32")

    # reset is refused from socket.io; an unknown action is dropped; abort
    # while idle does nothing.
    mark = len(esp.snapshot())
    for action in ("reset", "not-an-action", "abort"):
        await dial.emit("action", action)
    await asyncio.sleep(0.5)
    leaked = [
        command
        for command in (b"action,reset", b"action,not-an-action", b"action,stop", b"action,home")
        if command in written_since(mark)
    ]
    if leaked:
        failures.append(f"socket.io actions reached the ESP32 that must not: {leaked}")

    # calibrate sends the known weight and the current reading (0.0: the
    # silent ESP never reported a weight).
    mark = len(esp.snapshot())
    await dial.emit("calibrate", True)
    expected = b"action,calibration,100.0,0.0\x03"
    if not await _wait_for(lambda: expected in written_since(mark), 3):
        failures.append(f"socket.io calibrate did not write {expected!r}")

    # A hovered profile is relayed to every other client, not back to the sender.
    hover = {"id": "05051ed3-9996-43e8-9da6-963f2b31d481", "type": "profile", "from": "dial"}
    await dial.emit("profileHover", hover)
    if not await _wait_for(lambda: ("profileHover", hover) in received["app"], 3):
        failures.append("profileHover was not relayed to the other client")
    if ("profileHover", hover) in received["dial"]:
        failures.append("profileHover was echoed back to its sender")

    # A notification reaches the clients and is acknowledged over socket.io.
    # The ESP32 raises it, as it would for a scale or water warning.
    esp.send_line("Notify,generic,boot harness notification")

    def delivered():
        for event, data in received["app"]:
            if event == "notification" and "boot harness notification" in str(data):
                return json.loads(data)["id"]
        return None

    if not await _wait_for(lambda: delivered() is not None, 5):
        failures.append("a new notification was not emitted to the clients")
    else:
        notification_id = delivered()
        await app.emit("notification", json.dumps({"id": notification_id, "response": "Ok"}))

        def pending():
            _, body = _http_get(f"{base}/api/v1/notifications")
            return [entry["id"] for entry in json.loads(body or b"[]")]

        if not await _wait_for(lambda: notification_id not in pending(), 3):
            failures.append("an acknowledged notification is still listed as pending")

    for client in clients.values():
        await client.disconnect()


def _uart_checks(esp: FakeEsp | None, report: dict):
    if esp is None:
        return
    written = esp.snapshot()
    report["uart_written"] = written.decode(errors="replace")
    greeting = b"\x03action,info\x03"
    if not written.startswith(greeting):
        report["failures"].append(f"UART did not start with {greeting!r}: {written[:40]!r}")


def _database_checks(data_dir: Path, report: dict):
    database = data_dir / "meticulous-user" / "history" / "history.sqlite"
    try:
        with sqlite3.connect(database) as connection:
            row = connection.execute("SELECT version_num FROM alembic_version").fetchone()
        revision = row[0] if row else None
    except sqlite3.Error as error:
        revision = f"unreadable: {error}"
    report["db_revision"] = revision
    if revision != DB_VERSION_REQUIRED:
        report["failures"].append(f"history DB is at {revision}, the backend requires {DB_VERSION_REQUIRED}")


def _finish(report: dict, backend: Backend | None, report_path: str, host_log: Path | None) -> int:
    errors = backend.errors() if backend else []
    report["errors_logged"] = errors
    report["host_commands"] = host_log.read_text().splitlines() if host_log and host_log.exists() else []
    if errors:
        report["failures"].append(f"{len(errors)} record(s) logged at ERROR while booting")
    Path(report_path).write_text(json.dumps(report, indent=2, default=str))
    for failure in report["failures"]:
        print(f"BOOT FAILURE: {failure}", file=sys.stderr)
    for record in errors:
        print(f"ERROR RECORD: {record}", file=sys.stderr)
    return 0 if not report["failures"] else 1


def _default_binary(package_root: Path | None) -> Path:
    if package_root is not None and Path("/usr/bin/meticulous-backend").exists():
        return Path("/usr/bin/meticulous-backend")
    for profile in ("release", "debug"):
        candidate = REPO_ROOT / "target" / profile / "meticulous-backend"
        if candidate.exists():
            return candidate
    return Path("/usr/bin/meticulous-backend")


def run(args) -> int:
    report = {"failures": [], "boot": args.boot_index}
    data_dir = Path(args.data_dir).resolve()
    code_root = Path(args.package_root).resolve() if args.package_root else REPO_ROOT
    binary = Path(args.binary).resolve() if args.binary else _default_binary(args.package_root and code_root)
    report["binary"] = str(binary)
    port = _free_port()
    base = f"http://127.0.0.1:{port}"
    env = _environment(data_dir, code_root, port)
    host_log = _install_host_commands(data_dir, args.boot_index, env)
    report["host_commands_log"] = str(host_log)

    esp = None
    if args.esp_uart:
        env["METICULOUS_ESP_UART"] = args.esp_uart
    else:
        esp = FakeEsp()
        env["METICULOUS_ESP_UART"] = esp.device

    backend = Backend(binary, env, data_dir / f"backend-{args.boot_index}.log")
    report["backend_log"] = str(backend.log_path)

    if args.serve:
        stop = threading.Event()
        signal.signal(signal.SIGTERM, lambda *_: stop.set())
        signal.signal(signal.SIGINT, lambda *_: stop.set())
        if _wait_until_serving(base, backend, report):
            Path(args.serve).write_text(json.dumps({"port": port, "pid": os.getpid(), "data_dir": str(data_dir)}))
            while not stop.is_set() and backend.process.poll() is None:
                stop.wait(0.5)
        backend.stop()
        _uart_checks(esp, report)
        _database_checks(data_dir, report)
        return _finish(report, backend, args.report, host_log)

    def watchdog():
        report["failures"].append(f"boot did not finish within {BOOT_TIMEOUT_SECONDS:.0f} s")
        backend.stop()
        os._exit(_finish(report, backend, args.report, host_log))

    timer = threading.Timer(BOOT_TIMEOUT_SECONDS, watchdog)
    timer.daemon = True
    timer.start()
    try:
        if _wait_until_serving(base, backend, report):
            _http_checks(base, binary, report)
            asyncio.run(_socketio_checks(base, report))
            asyncio.run(_socketio_inbound_checks(base, esp, report))
            _uart_checks(esp, report)
            _database_checks(data_dir, report)
    except Exception:
        import traceback

        report["failures"].append("checks crashed:\n" + traceback.format_exc())
    finally:
        timer.cancel()
        backend.stop()
    return _finish(report, backend, args.report, host_log)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--data-dir", required=True, help="throwaway root for user data")
    parser.add_argument("--report", required=True, help="where to write the JSON report")
    parser.add_argument("--binary", help="the backend binary (default: the installed one, else target/)")
    parser.add_argument(
        "--package-root",
        help="the package's read-only data, e.g. /opt/meticulous-backend (default: this checkout)",
    )
    parser.add_argument(
        "--esp-uart",
        help="serial device of a real firmware, e.g. fika-sil's --pty-link, instead of the fake UART",
    )
    parser.add_argument(
        "--serve",
        metavar="READY_FILE",
        help='skip the checks and keep the backend running until SIGTERM; READY_FILE gets {"port", "pid", "data_dir"} once it serves',
    )
    parser.add_argument("--boot-index", type=int, default=1, help=argparse.SUPPRESS)
    sys.exit(run(parser.parse_args()))


if __name__ == "__main__":
    main()
