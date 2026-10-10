"""Boot the real backend entry point with only the machine's hardware replaced.

Runs ``back.run()``, the function the systemd service starts, in production
mode (``BACKEND=FIKA``), and checks from inside the running server that the
backend came up:

* the HTTP server answers and every read-only GET route answers below 500,
* a Socket.IO client receives the ``status`` stream with its documented keys,
* the ESP32 was greeted over the UART (``\\x03`` then ``action,info``),
* the history database was migrated to ``DB_VERSION_REQUIRED``,
* nothing logged at ERROR or above while booting.

What is replaced is the hardware and the host OS the machine image provides,
each at its boundary: the ESP32 UART, GPIO, I2C, systemd and BlueZ (their
D-Bus services), NetworkManager, ``timedatectl``, ``chpasswd``, audio
playback, disk imaging and Sentry's network transport. Everything else runs
for real against throwaway directories, including the D-Bus system bus, which
the caller provides (``dbus-daemon --system``).

The harness runs as its own process so the backend's module-level singletons
and threads never leak into the pytest process. It writes a JSON report to
``--report`` and exits 0 only when every check passed. It needs the
machine's dependency group (pydbus needs PyGObject), so CI runs it with the
virtualenv from the built .deb; ``tests/test_boot.py`` runs it under pytest
when those dependencies are installed.

Usage: python tests/boot/boot_harness.py --data-dir DIR --report FILE
           [--package-root DIR]

Booting twice with the same --data-dir checks the restart of a machine that
already has its own config, profiles and history.
"""

import argparse
import asyncio
import json
import logging
import os
import socket
import sys
import threading
import time
import traceback
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
REPO_ROOT = HERE.parent.parent

# Seconds the whole boot plus checks may take before the watchdog fails it.
BOOT_TIMEOUT_SECONDS = float(os.getenv("BOOT_TIMEOUT_SECONDS", "90"))

# Keys every ``status`` event carries: ShotData.to_sio() plus what live()
# adds. The Dial and the mobile app read these by name.
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
KNOWN_ISSUES = {
    "/api/v1/history/debug.zip": {
        "bug": "500 on a machine without debug shots: the zip is opened inside "
        "DEBUG_HISTORY_PATH before anything creates that directory",
        "errors": [
            "Error compressing debug file: FileNotFoundError",
            "500 GET /api/v1/history/debug.zip",
        ],
    },
    "/api/v1/profile/load": {
        "bug": "GET without a profile id reaches LoadProfileHandler.get(profile_id) "
        "and raises TypeError instead of answering 404/405",
        "errors": [
            "Uncaught exception GET /api/v1/profile/load ",
            "500 GET /api/v1/profile/load ",
        ],
    },
}
_KNOWN_ERROR_PREFIXES = [
    prefix for issue in KNOWN_ISSUES.values() for prefix in issue["errors"]
]


def _free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def _prepare_environment(data_dir: Path, code_root: Path, port: int):
    """Point every path the backend writes to into ``data_dir``.

    Must run before any backend module is imported: most of them read their
    paths from the environment at import time.
    """
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
        "TIMEZONE_JSON_FILE_PATH": data_dir
        / "usr"
        / "share"
        / "zoneinfo"
        / "UI_timezones.json",
        "UPDATE_PATH": data_dir / "opt" / "meticulous-firmware",
        "DEFAULT_IMAGES": code_root / "images" / "default",
        "SYSTEM_SOUNDS": code_root / "sounds",
    }
    for key, value in paths.items():
        os.environ[key] = str(value)
    for key in ("REDACTION_KEY_PATH", "TIMEZONE_JSON_FILE_PATH", "DEVICE_UUID_CACHE_PATH"):
        Path(os.environ[key]).parent.mkdir(parents=True, exist_ok=True)
    os.environ["BACKEND"] = "FIKA"
    os.environ["PORT"] = str(port)
    os.environ.pop("SENTRY", None)
    os.environ.pop("DEBUG", None)


class FakeEspPort:
    """The ESP32's UART as pyserial exposes it: records writes, never answers."""

    def __init__(self):
        self.written = bytearray()
        self.in_waiting = 0
        self.is_open = True

    def write(self, data):
        self.written += data
        return len(data)

    def read(self, size=1):
        time.sleep(0.05)
        return b""

    def readline(self):
        time.sleep(0.05)
        return b""

    def reset_input_buffer(self):
        pass

    def reset_output_buffer(self):
        pass

    def flush(self):
        pass

    def close(self):
        self.is_open = False


class FakeFikaSerialConnection:
    """Stands in for FikaSerialConnection: no GPIO lines, a FakeEspPort as UART."""

    instances = []

    def __init__(self, device, *args, **kwargs):
        self.device = device
        self.port = FakeEspPort()
        FakeFikaSerialConnection.instances.append(self)

    def reset(self, *args, **kwargs):
        pass

    def sendUpdate(self, *args, **kwargs):
        return "firmware updates are not available in the boot harness"


class ErrorCollector(logging.Handler):
    def __init__(self):
        super().__init__(level=logging.ERROR)
        self.records = []

    def emit(self, record):
        message = record.getMessage()
        if any(message.startswith(prefix) for prefix in EXPECTED_BOOT_ERRORS):
            return
        if any(message.startswith(prefix) for prefix in _KNOWN_ERROR_PREFIXES):
            return
        detail = f"{record.name}: {message}"
        if record.exc_info:
            detail += "\n" + "".join(traceback.format_exception(*record.exc_info))
        self.records.append(detail)


# Host commands the machine image provides, replaced by recording stubs on
# PATH. Keyed by the first argument, each prints what the real tool prints for
# the call the backend makes at boot; any other call prints nothing and
# succeeds. Commands that change the host (reboot, mount, hostnamectl, ...)
# are stubbed so that a boot that calls one records it instead of acting on
# the CI runner. host_commands in the report lists every call.
HOST_COMMANDS = {
    "nmcli": {},
    "pactl": {},
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


def _install_host_commands(data_dir: Path, boot_index: int) -> Path:
    bin_dir = data_dir / "host-bin"
    bin_dir.mkdir(parents=True, exist_ok=True)
    log = data_dir / f"host-commands-{boot_index}.log"
    for name, outputs in HOST_COMMANDS.items():
        cases = "".join(
            f"    {argument}) printf '%s\\n' '{output}' ;;\n"
            for argument, output in outputs.items()
        )
        stub = bin_dir / name
        stub.write_text(
            "#!/bin/sh\n"
            f'echo "{name} $*" >> "{log}"\n'
            f'case "$1" in\n{cases}    *) ;;\nesac\n'
            "exit 0\n"
        )
        stub.chmod(0o755)
    os.environ["PATH"] = f"{bin_dir}{os.pathsep}{os.environ.get('PATH', '')}"
    return log


def _install_import_time_fakes():
    """Fakes that must exist before the backend modules are imported."""
    import sentry_sdk

    real_client = sentry_sdk.Client
    real_init = sentry_sdk.init

    # back.py initializes Sentry at import time when BACKEND is FIKA and
    # machine.py builds a second client for the ESP32 project. Both stay real,
    # integrations included, with the DSN dropped so nothing is sent. The DSN
    # is the only positional parameter of either, hence args[1:].
    def offline_client(*args, **kwargs):
        kwargs["dsn"] = None
        return real_client(*args[1:], **kwargs)

    def offline_init(*args, **kwargs):
        kwargs["dsn"] = None
        return real_init(*args[1:], **kwargs)

    sentry_sdk.Client = offline_client
    sentry_sdk.init = offline_init


def _hardware_patches(issue_file: Path):
    """Patches at the hardware boundary, applied around back.run()."""
    gpio_chip = mock.MagicMock(name="gpiod.chip")

    return [
        # ESP32 UART and its enable/boot GPIO lines.
        mock.patch("machine.FikaSerialConnection", FakeFikaSerialConnection),
        # Audio enable GPIO and playback.
        mock.patch("sounds.gpiod.chip", gpio_chip, create=True),
        mock.patch("sounds.playsound", mock.MagicMock(name="playsound")),
        # systemd's D-Bus service. The bus itself is real, so DBusMonitor
        # subscribes to the RAUC, Hawkbit and USB signals for real.
        mock.patch("system_services.SystemBus"),
        # USB-C PD controller on I2C.
        mock.patch("usb.PTN5150H"),
        # The console banner the root password is written into.
        mock.patch("ssh_manager.SSHManager.ISSUE_PATH", str(issue_file)),
        # BlueZ GATT server.
        mock.patch("ble_gatt.GATTServer.getServer"),
        # NetworkManager: the Wi-Fi manager starts on its own thread on a machine.
        mock.patch("backend.start_wifi_manager_in_background"),
        # Never touch block devices.
        mock.patch("imager.DiscImager.needsImaging", return_value=False),
    ]


async def _http_checks(port: int, report: dict):
    from tornado.httpclient import AsyncHTTPClient, HTTPClientError
    from tornado.web import RequestHandler

    from api.api import API

    client = AsyncHTTPClient()
    base = f"http://127.0.0.1:{port}"

    deadline = time.monotonic() + 15
    while True:
        try:
            # Any answer means the server is up; the sweep judges the status.
            await client.fetch(f"{base}/api/v1/settings", raise_error=False)
            break
        except (ConnectionError, OSError):
            if time.monotonic() > deadline:
                report["failures"].append("HTTP server never accepted a connection")
                return
            await asyncio.sleep(0.2)

    swept = {}
    for route in API.get_routes():
        pattern, handler = route[0], route[1]
        if "(" in pattern or pattern in SWEEP_SKIP:
            continue
        if handler.get is RequestHandler.get:
            continue  # not a GET route
        path = pattern.replace("[/]*", "")
        try:
            response = await client.fetch(
                f"{base}{path}", raise_error=False, request_timeout=20
            )
            code = response.code
        except HTTPClientError as error:
            code = error.code
        except Exception as error:  # timeouts, resets
            code = f"{type(error).__name__}: {error}"
        swept[path] = code
        failed = not isinstance(code, int) or code >= 500
        if path in KNOWN_ISSUES:
            if not failed:
                report["failures"].append(
                    f"GET {path} answered {code}: the known issue is fixed, "
                    "remove its KNOWN_ISSUES entry"
                )
        elif failed:
            report["failures"].append(f"GET {path} answered {code}")
    report["routes"] = swept
    report["known_issues"] = {
        path: issue["bug"] for path, issue in KNOWN_ISSUES.items() if path in swept
    }
    for path in KNOWN_ISSUES.keys() - swept.keys():
        report["failures"].append(f"KNOWN_ISSUES lists {path}, which is no longer a GET route")


async def _socketio_checks(port: int, report: dict):
    import socketio

    received = {}
    sio = socketio.AsyncClient(reconnection=False)

    @sio.on("*")
    async def any_event(event, data=None):
        received.setdefault(event, data)

    try:
        await sio.connect(f"http://127.0.0.1:{port}", transports=["websocket"], wait_timeout=10)
    except Exception as error:
        report["failures"].append(f"Socket.IO connect failed: {error}")
        return

    deadline = time.monotonic() + 10
    while "status" not in received and time.monotonic() < deadline:
        await asyncio.sleep(0.1)
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


def _uart_checks(report: dict):
    if not FakeFikaSerialConnection.instances:
        report["failures"].append("Machine.init never opened the ESP32 UART")
        return
    written = bytes(FakeFikaSerialConnection.instances[0].port.written)
    report["uart_written"] = written.decode(errors="replace")
    greeting = b"\x03action,info\x03"
    if not written.startswith(greeting):
        report["failures"].append(f"UART did not start with {greeting!r}: {written[:40]!r}")


def _database_checks(report: dict):
    from alembic.runtime.migration import MigrationContext
    from sqlalchemy import create_engine

    from config import DATABASE_URL
    from db_migration_updater import DB_VERSION_REQUIRED

    engine = create_engine(DATABASE_URL)
    try:
        with engine.connect() as connection:
            revision = MigrationContext.configure(connection).get_current_revision()
    finally:
        engine.dispose()
    report["db_revision"] = revision
    if revision != DB_VERSION_REQUIRED:
        report["failures"].append(
            f"history DB is at {revision}, the backend requires {DB_VERSION_REQUIRED}"
        )


def _boot(args):
    report = {"failures": [], "boot": args.boot_index}
    errors = ErrorCollector()

    def watchdog():
        report["failures"].append(f"boot did not finish within {BOOT_TIMEOUT_SECONDS:.0f} s")
        _finish(report, errors, args.report)

    timer = threading.Timer(BOOT_TIMEOUT_SECONDS, watchdog)
    timer.daemon = True
    timer.start()

    try:
        _run_backend(args, report, errors)
    except BaseException:
        report["failures"].append("boot crashed:\n" + traceback.format_exc())
    timer.cancel()
    _finish(report, errors, args.report)


def _run_backend(args, report: dict, errors: ErrorCollector):
    data_dir = Path(args.data_dir).resolve()
    code_root = Path(args.package_root).resolve() if args.package_root else REPO_ROOT
    port = _free_port()
    _prepare_environment(data_dir, code_root, port)
    sys.path.insert(0, str(code_root))
    os.chdir(code_root)
    # backend.main() hands sys.argv to tornado's option parser, as the
    # launcher's "$@" would: start it with no options.
    sys.argv = [str(code_root / "back.py")]

    host_command_log = _install_host_commands(data_dir, args.boot_index)
    report["host_commands_log"] = str(host_command_log)
    _install_import_time_fakes()
    logging.getLogger().addHandler(errors)

    import tornado.ioloop

    import back  # noqa: E402  (environment and fakes first)

    report["code_root"] = str(Path(back.__file__).resolve().parent)

    io_loop = tornado.ioloop.IOLoop.current()

    async def checks():
        try:
            await _http_checks(port, report)
            await _socketio_checks(port, report)
            _uart_checks(report)
            _database_checks(report)
        except Exception:
            report["failures"].append("checks crashed:\n" + traceback.format_exc())
        finally:
            io_loop.stop()

    io_loop.add_callback(checks)

    patches = _hardware_patches(data_dir / "etc-issue")
    for patch in patches:
        patch.start()
    try:
        back.run()
    except SystemExit as error:
        report["failures"].append(f"back.run() exited with {error.code}")
    finally:
        for patch in reversed(patches):
            patch.stop()


def _finish(report: dict, errors: ErrorCollector, report_path: str):
    report["errors_logged"] = list(errors.records)
    log = Path(report.get("host_commands_log", "/nonexistent"))
    report["host_commands"] = log.read_text().splitlines() if log.exists() else []
    if errors.records:
        report["failures"].append(
            f"{len(errors.records)} record(s) logged at ERROR while booting"
        )
    Path(report_path).write_text(json.dumps(report, indent=2, default=str))
    for failure in report["failures"]:
        print(f"BOOT FAILURE: {failure}", file=sys.stderr)
    for record in errors.records:
        print(f"ERROR RECORD: {record}", file=sys.stderr)
    sys.stdout.flush()
    sys.stderr.flush()
    # The backend's threads are not daemons and never return: leave hard.
    os._exit(0 if not report["failures"] else 1)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--data-dir", required=True, help="throwaway root for user data")
    parser.add_argument("--report", required=True, help="where to write the JSON report")
    parser.add_argument(
        "--package-root",
        help="backend code to boot, e.g. /opt/meticulous-backend (default: this checkout)",
    )
    parser.add_argument("--boot-index", type=int, default=1, help=argparse.SUPPRESS)
    _boot(parser.parse_args())


if __name__ == "__main__":
    main()
