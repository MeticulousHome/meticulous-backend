#!/usr/bin/env python3
# Like tests/, which the mypy ratchet excludes: a test tool that swaps modules,
# methods and clocks for recording stubs, which mypy rightly calls type errors.
# mypy: ignore-errors
"""Generate the parity goldens of crates/esp from the current Python.

Run from the repository root:

    uv run python crates/esp/tools/gen_goldens.py

It runs the Python itself, never a transcription of it:

- esp_serial/data.py (from_args, from_color_coded_args, to_args, to_sio,
  to_sio_sensors, to_dict, clone_with_time_and_state, and the objects the
  emulator builds from esp_serial/connection/emulated.*.json);
- the line dispatch of Machine._read_data: the `match (data_str_sensors)`
  statement and the boot-banner expression are cut out of machine.py with
  `ast` and executed against recording stubs, so a change to machine.py
  changes the goldens;
- esp_observability.py over seeded random scripts;
- Machine._parseVersionString (cut out of machine.py) with packaging.version;
- monitoring/motor_power_monitoring.py with a scripted clock;
- the UART writes of the real Machine methods (machine.py imported with the
  stubs tests/conftest.py uses) and of backend.py's calibrate handler;
- ESPToolWrapper.get_version_from_firmware over synthetic firmware images.

Values are written as JSON with Python's types (1 vs 1.0); non-finite floats
as {"nonfinite": "inf" | "-inf" | "nan"}; to_sio payloads as the json.dumps
text with non-finite floats replaced by null (the Rust backend's convention).
crates/esp/tests/golden.rs replays every case. Regenerate whenever one of
those Python files changes.
"""

import ast
import asyncio
import dataclasses
import json
import logging
import math
import os
import random
import struct
import sys
import tempfile
import textwrap
import threading
import types
from enum import Enum
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
OUT = REPO / "crates" / "esp" / "testdata" / "goldens"
sys.path.insert(0, str(REPO))

# --------------------------------------------------------------------------
# Stubs. log.py needs the log_redactor submodule; the generator only needs
# loggers. named_thread would start the motor energy save thread at import.
# --------------------------------------------------------------------------

log_stub = types.ModuleType("log")


class _StubMeticulousLogger:
    @staticmethod
    def getLogger(name=None, *_args, **_kwargs):
        logger = logging.getLogger(f"gen_goldens.{name}")
        logger.propagate = False
        logger.addHandler(logging.NullHandler())
        return logger

    set_redaction_value_provider = staticmethod(lambda *_a, **_k: None)
    invalidate_redaction_values = staticmethod(lambda *_a, **_k: None)


setattr(log_stub, "MeticulousLogger", _StubMeticulousLogger)
setattr(log_stub, "LOGLEVEL", logging.INFO)
sys.modules["log"] = log_stub

named_thread_stub = types.ModuleType("named_thread")


class _InertThread(threading.Thread):
    def __init__(self, name, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.name = name

    def start(self):
        pass


setattr(named_thread_stub, "NamedThread", _InertThread)
sys.modules["named_thread"] = named_thread_stub

_motor_dir = tempfile.mkdtemp(prefix="esp-goldens-motor-")
os.environ["MOTOR_ENERGY_PATH"] = _motor_dir

import tests.conftest  # noqa: E402,F401  (the machine-only package stubs)
from esp_serial.data import (  # noqa: E402
    ButtonEventData,
    ESPInfo,
    HeaterTimeoutInfo,
    MachineNotify,
    SensorData,
    ShotData,
)
import esp_observability  # noqa: E402
from esp_observability import ESPObservability  # noqa: E402
from packaging import version as packaging_version  # noqa: E402
import monitoring.motor_power_monitoring as motor_module  # noqa: E402
from esp_serial.connection.emulation_data import EmulationData  # noqa: E402

RNG = random.Random(20261024)


class BigInt(Exception):
    pass


def enc(value):
    """A Python value as JSON that keeps int/float apart."""
    if value is None or isinstance(value, (bool, str)):
        return value
    if isinstance(value, Enum):
        return value.name
    if isinstance(value, int):
        if not -(2**63) <= value < 2**63:
            raise BigInt(value)
        return value
    if isinstance(value, float):
        if math.isfinite(value):
            return value
        return {"nonfinite": repr(value)}
    if isinstance(value, (list, tuple)):
        return [enc(v) for v in value]
    if isinstance(value, dict):
        return {str(k): enc(v) for k, v in value.items()}
    if dataclasses.is_dataclass(value):
        return {f.name: enc(getattr(value, f.name)) for f in dataclasses.fields(value)}
    raise TypeError(f"cannot encode {type(value)}")


def finite_or_none(value):
    if isinstance(value, float) and not math.isfinite(value):
        return None
    if isinstance(value, dict):
        return {k: finite_or_none(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [finite_or_none(v) for v in value]
    return value


def sio_text(payload):
    return json.dumps(finite_or_none(payload))


def fields(obj):
    if obj is None:
        return None
    if isinstance(obj, ButtonEventData):
        return {
            "event": obj.event.name,
            "time_since_last_event": enc(obj.time_since_last_event),
        }
    return enc(obj)


def render(obj):
    """Fields, to_args and the Socket.IO payload of a parsed message."""
    if obj is None:
        return None
    out = {"fields": fields(obj)}
    if isinstance(obj, (SensorData, ShotData, ESPInfo)):
        out["to_args"] = obj.to_args()
    if isinstance(obj, SensorData):
        out["to_sio"] = sio_text(obj.to_sio_sensors())
    elif isinstance(obj, (ShotData, ESPInfo, ButtonEventData)):
        out["to_sio"] = sio_text(obj.to_sio())
    elif isinstance(obj, HeaterTimeoutInfo):
        out["to_sio"] = sio_text(obj.to_dict())
    return out


# --------------------------------------------------------------------------
# Field corpora
# --------------------------------------------------------------------------

NUMBERS = [
    "1.5",
    "0",
    "0.0",
    "-0.0",
    "-2.25",
    "93.5",
    "100",
    "1e-5",
    "2E3",
    "+3.5",
    "1_0.5",
    "1__0",
    "_1",
    "1_",
    "nan",
    "NaN",
    "-nan",
    "inf",
    "-inf",
    "Infinity",
    "-Infinity",
    "infinity",
    "1e400",
    "-1e400",
    " 2.5 ",
    "\t7\r\n",
    "",
    " ",
    "abc",
    "0x10",
    "1.2.3",
    "9999+++",
    "12345678901234567",
    "0.1",
    "1e16",
    "123456789.123456789",
    "5e-324",
    "true",
    "S",
    "1,5",
]
WORDS = [
    "",
    "idle",
    "heating",
    "purge",
    "Purge",
    "Home",
    "home",
    "IDLE",
    "My Espresso",
    "My%20Profile",
    "caf%C3%A9",
    "%E2%82%AC5",
    "%zz",
    "%fF",
    "100%",
    "a;b",
    "x\r",
    "\r\n",
]
KINDS = [
    "none",
    "None",
    "Flow",
    "Pressure",
    "Power",
    "Piston",
    "temperature",
    "FLOW",
    "",
    "none\r",
]
BOOLS = ["true", "false", "TRUE", "True", "yes", "", "1", "S", "U"]


def pick(options):
    return RNG.choice(options)


def sensor_args():
    n = RNG.choice([23, 23, 23, 24, 22, 21, 25, 30, 2, 0])
    args = [
        pick(NUMBERS[:12]) if RNG.random() < 0.8 else pick(NUMBERS) for _ in range(min(n, 20))
    ]
    if n > 20:
        args.append(pick(BOOLS))
    while len(args) < n:
        args.append(pick(NUMBERS))
    return args


def shot_args():
    n = RNG.choice([13, 13, 13, 18, 7, 8, 12, 14, 5, 4])
    args = [pick(NUMBERS) for _ in range(3)]
    args.append(pick(["S", "U", "S\r\n", "s", ""]))
    args.append(pick(NUMBERS))
    args.append(pick(WORDS))
    args.append(pick(WORDS))
    args += [pick(KINDS), pick(NUMBERS), pick(KINDS), pick(NUMBERS), pick(BOOLS), pick(NUMBERS)]
    args += [pick(WORDS), pick(NUMBERS), pick(WORDS), pick(NUMBERS), pick(["Idle", "Brewing"])]
    return args[:n]


UUIDS = [
    "123e4567-e89b-42d3-a456-426614174000",
    "123E4567-E89B-42D3-A456-426614174000",
    "123e4567-e89b-12d3-a456-426614174000",
    "123e4567-e89b-42d3-c456-426614174000",
    "123e4567e89b42d3a456426614174000",
    "123e4567-e89b-42d3-a456-426614174000\n",
    "",
]


def esp_info_args():
    n = RNG.choice([3, 8, 9, 10, 11, 11, 12, 12, 13, 2, 1, 0, 5])
    args = [
        pick(["1.2.3", "0.9.10-sil", "", "v2", "0.2.24-251-gd519a85"]),
        pick(["2", "off", "notanint", " 7 ", "+1", "1_0", "-3", "4.0", ""]),
        pick(NUMBERS),
        pick(["black", "", "NOT_ASSIGNED", "white"]),
        pick(["SN123", "", "SIL-TM-0001"]),
        pick(["0025", ""]),
        pick(["2025-11-14", ""]),
        pick(["acaia", "none", "generic", ""]),
        pick(NUMBERS),
        pick(BOOLS),
        pick(["after_retraction", "before_retraction", ""] + UUIDS),
        pick(UUIDS),
        "extra",
    ]
    return args[:n]


def button_args():
    tokens = [
        "CW",
        "CCW",
        "push",
        "pu_d",
        "elng",
        "tare",
        "ta_d",
        "ta_l",
        "ta_sl",
        "strt",
        "cntx",
        "encoder_button_pressed",
        "encoder_button_released",
        "tare_pressed",
        "tare_released",
        "context_pressed",
        "context_released",
        "spare_1_pressed",
        "unknown",
        "UNKNOWN",
        "encoder_clockwise",
        "Tare_Long",
        "cw",
        "Push",
        "",
        "ENCODER_PUSH ",
        "doesnotexist",
    ]
    n = RNG.choice([1, 2, 2, 2, 3, 0])
    args = [
        pick(tokens),
        pick(["150", "0", "9999+++", "-5", " 12 ", "1_000", "1.5", "", "abc", "05"]),
        "x",
    ]
    return args[:n]


def heater_args():
    n = RNG.choice([4, 4, 4, 3, 5, 0])
    return [pick(NUMBERS[:24]) for _ in range(n)]


def data_cases():  # noqa: C901
    cases = []

    def add(kind, args, build):
        try:
            result = render(build(args))
        except BigInt:
            return
        cases.append({"kind": kind, "args": args, "result": result})

    def shot_or_raise(args):
        try:
            return ShotData.from_args(args)
        except IndexError:
            return "raises"

    for _ in range(250):
        add("sensors", sensor_args(), SensorData.from_args)
    for _ in range(350):
        args = shot_args()
        try:
            result = ShotData.from_args(args)
        except IndexError:
            cases.append({"kind": "shot", "args": args, "raises": "IndexError"})
            continue
        try:
            cases.append({"kind": "shot", "args": args, "result": render(result)})
        except BigInt:
            pass
    for _ in range(250):
        add("esp_info", esp_info_args(), ESPInfo.from_args)
    for _ in range(150):
        add("button", button_args(), ButtonEventData.from_args)
    for args in [[], ["warning"], ["warning", "Low water"], ["a", "b", "c"]]:
        add("notify", args, MachineNotify.from_args)
    for _ in range(80):
        args = heater_args()
        try:
            result = HeaterTimeoutInfo.from_args(args)
        except ValueError as error:
            cases.append({"kind": "heater", "args": args, "raises": str(error)})
            continue
        cases.append({"kind": "heater", "args": args, "result": render(result)})

    # Color-coded sensors: the regex replaces labels with commas.
    colors = ["31", "32", "33", "34", "35", "36"]
    for _ in range(40):
        values = sensor_args()
        text = ""
        for value in values:
            color = pick(colors + ["39", "37"]) if RNG.random() < 0.95 else "31"
            label = pick(["ext1", "t_bar_up", "a0", "X", "", "mp_2", "ws"])
            text += f"\x1b[1;{color}m {label}\x1b[0m{value}"
        add("sensors_color", text, SensorData.from_color_coded_args)
    for text in ["", ",", "\x1b[1;31m a\x1b[0m", "\x1b[1;31m A\x1b[0m1", "1,2,3"]:
        add("sensors_color", text, SensorData.from_color_coded_args)
    return cases


def construct_cases():
    """Objects built from keyword arguments, as the emulator does."""
    cases = []
    with open(REPO / "esp_serial/connection/emulated.shot.json") as f:
        shot = json.load(f)
    lines = EmulationData.shotToEmulation(shot)
    samples = [s for s in shot["data"] if s["time"] > 0]
    step = max(1, len(samples) // 60)
    for sample in samples[::step]:
        sensor = sample["sensors"]
        data = sample["shot"]
        kwargs = {
            k: sensor[k]
            for k in sensor
            if k in SensorData.__dataclass_fields__ and k != "motor_thermistor"
        }
        kwargs["water_status"] = sensor.get("water_status", False)
        obj = SensorData(**kwargs)
        cases.append(
            {
                "type": "SensorData",
                "kwargs": enc(kwargs),
                "to_args": obj.to_args(),
                "to_sio": sio_text(obj.to_sio_sensors()),
            }
        )
        active = data["setpoints"]["active"]
        main_setpoint = data["setpoints"].get(active, None) if active is not None else None
        kwargs = dict(
            pressure=data["pressure"],
            flow=data["flow"],
            weight=data["weight"],
            gravimetric_flow=data["gravimetric_flow"],
            temperature=data.get("temperature", sensor["tube"]),
            profile=shot["profile_name"],
            status=sample["status"],
            main_controller_kind=active,
            main_setpoint=main_setpoint,
        )
        obj = ShotData(**kwargs)
        cases.append(
            {
                "type": "ShotData",
                "kwargs": enc(kwargs),
                "to_args": obj.to_args(),
                "to_sio": sio_text(obj.to_sio()),
            }
        )
    extra = [
        ("ShotData", dict(temperature=23.2, state="idle", status="idle", profile="idle")),
        (
            "ShotData",
            dict(
                main_controller_kind="Flow",
                main_setpoint=None,
                aux_controller_kind="Pressure",
                aux_setpoint=9,
                is_aux_controller_active=True,
                pressure="NaN",
                flow=0,
            ),
        ),
        (
            "ShotData",
            dict(
                status=None,
                profile=None,
                weight=float("inf"),
                time=1234,
                profile_time=99,
                is_extracting=True,
            ),
        ),
        ("ShotData", {}),
        ("SensorData", {}),
        (
            "SensorData",
            dict(
                motor_thermistor="NaN",
                weight_prediction=float("-inf"),
                water_status=True,
                external_1=0,
                adc_3=-0.0,
            ),
        ),
        ("ESPInfo", {}),
        ("ESPInfo", dict(tareBehavior="after_retraction")),
        ("ESPInfo", dict(deviceUUID=UUIDS[0], deviceUUIDSupported=True)),
        (
            "ESPInfo",
            dict(
                tareBehavior="",
                deviceUUID="",
                deviceUUIDSupported=True,
                mainVoltage=float("nan"),
            ),
        ),
    ]
    builders = {"ShotData": ShotData, "SensorData": SensorData, "ESPInfo": ESPInfo}
    for type_name, kwargs in extra:
        obj = builders[type_name](**kwargs)
        payload = obj.to_sio_sensors() if type_name == "SensorData" else obj.to_sio()
        cases.append(
            {
                "type": type_name,
                "kwargs": enc(kwargs),
                "to_args": obj.to_args(),
                "to_sio": sio_text(payload),
            }
        )
    clones = []
    for args in [lines[0].split(",")[1:], lines[200].split(",")[1:]]:
        base = ShotData.from_args(args)
        for start, brewing, profile_time in [(0, False, 0), (1234, True, 1200), (5, True, -1)]:
            obj = base.clone_with_time_and_state(start, brewing, profile_time)
            clones.append(
                {
                    "args": args,
                    "clone": [start, brewing, profile_time],
                    "to_sio": sio_text(obj.to_sio()),
                }
            )
    return cases, clones, lines


# --------------------------------------------------------------------------
# The line dispatch of Machine._read_data, executed from machine.py's source
# --------------------------------------------------------------------------

MACHINE_SRC = (REPO / "machine.py").read_text()
MACHINE_TREE = ast.parse(MACHINE_SRC)


def find_function(tree, name):
    for node in ast.walk(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and node.name == name:
            return node
    raise LookupError(name)


def segment(src, node):
    return textwrap.dedent(ast.get_source_segment(src, node, padded=True))


READ_DATA = find_function(MACHINE_TREE, "_read_data")
MATCH_NODE = next(
    n
    for n in ast.walk(READ_DATA)
    if isinstance(n, ast.Match)
    and isinstance(n.subject, ast.Name)
    and n.subject.id == "data_str_sensors"
)
BANNER_NODE = next(
    n
    for n in ast.walk(READ_DATA)
    if isinstance(n, ast.Assign)
    and any(isinstance(t, ast.Name) and t.id == "is_boot_banner" for t in n.targets)
)


class Recorder:
    def __init__(self):
        self.reset()

    def reset(self):
        self.calls = []
        self.logs = []
        self.sentry = []
        self.emits = []
        self.smoke = []


REC = Recorder()


class RecordingLogger:
    def _log(self, level, msg, *args, **_kwargs):
        REC.logs.append((level, msg % args if args else msg))

    def debug(self, msg, *a, **k):
        self._log("debug", msg, *a, **k)

    def info(self, msg, *a, **k):
        self._log("info", msg, *a, **k)

    def warning(self, msg, *a, **k):
        self._log("warning", msg, *a, **k)

    def error(self, msg, *a, **k):
        self._log("error", msg, *a, **k)


class StubSio:
    async def emit(self, event, payload):
        REC.emits.append((event, payload))


class StubMachine:
    esp_info = None
    firmware_running = None
    heater_timeout_info = None
    _sio = StubSio()

    @staticmethod
    def handleDeviceUUIDResponse(response):
        REC.calls.append(("handleDeviceUUIDResponse", response))

    @staticmethod
    def handleTareBehaviorNVSResponse(status):
        REC.calls.append(("handleTareBehaviorNVSResponse", status))

    @staticmethod
    def _handleMileageRequest():
        REC.calls.append(("_handleMileageRequest",))

    @staticmethod
    def _handleNvsResponse(key, value):
        REC.calls.append(("_handleNvsResponse", key, value))


class StubScope:
    def __init__(self):
        self.contexts = {}
        self.tags = {}

    def __enter__(self):
        return self

    def __exit__(self, *_a):
        return False

    def set_context(self, key, value):
        self.contexts[key] = value

    def set_tag(self, key, value):
        self.tags[key] = value

    def set_client(self, _client):
        pass

    def capture_event(self, event):
        REC.sentry.append({"event": event, "contexts": self.contexts, "tags": self.tags})


DISPATCH_NS = {
    "Machine": StubMachine,
    "ButtonEventData": ButtonEventData,
    "ShotData": ShotData,
    "SensorData": SensorData,
    "ESPInfo": ESPInfo,
    "MachineNotify": MachineNotify,
    "HeaterTimeoutInfo": HeaterTimeoutInfo,
    "logger": RecordingLogger(),
    "SmokeValidationManager": types.SimpleNamespace(
        observe_esp_log=lambda m: REC.smoke.append(m)
    ),
    "sentry_sdk": types.SimpleNamespace(new_scope=StubScope),
    "ESPSentryClient": None,
}
_prelude = (
    "\n".join(
        f"{name} = None"
        for name in [
            "button_event",
            "sensor",
            "data",
            "info",
            "notify",
            "boot_reason",
            "valid_message_type",
        ]
    )
    + "\nis_valid_message = True\n"
)
_dispatch_src = (
    "async def _dispatch(data_str, data_str_sensors, previous_preheat_remaining):\n"
    + textwrap.indent(
        _prelude + segment(MACHINE_SRC, MATCH_NODE) + "\nreturn locals()\n", "    "
    )
    + "\n\ndef _banner(data_str):\n"
    + textwrap.indent(segment(MACHINE_SRC, BANNER_NODE) + "\nreturn is_boot_banner\n", "    ")
)
exec(compile(_dispatch_src, "machine.py:_read_data", "exec"), DISPATCH_NS)
LOOP = asyncio.new_event_loop()


def dispatch_line(line):  # noqa: C901
    REC.reset()
    boot = DISPATCH_NS["_banner"](line)
    fields_ = line.strip("\r\n").split(",")
    try:
        loc = LOOP.run_until_complete(DISPATCH_NS["_dispatch"](line, fields_, None))
    except Exception as error:  # noqa: BLE001  (what escapes _read_data)
        return {
            "line": line,
            "boot_banner": boot,
            "kind": "uncaught",
            "error": type(error).__name__,
        }

    out = {
        "line": line,
        "boot_banner": boot,
        "type": loc["valid_message_type"],
        "valid": loc["is_valid_message"],
    }
    vmt = loc["valid_message_type"]
    calls = REC.calls
    if vmt is None:
        out["kind"] = "unknown"
    elif vmt == "Event":
        out["kind"] = "event"
        out["result"] = render(loc["button_event"])
    elif vmt == "Data":
        out["kind"] = "data"
        out["result"] = render(loc["data"])
    elif vmt == "Sensors":
        out["kind"] = "sensors"
        out["result"] = render(loc["sensor"])
    elif vmt == "ESPInfo":
        out["kind"] = "esp_info"
        out["result"] = render(loc["info"])
    elif vmt == "ESPBoot":
        out["kind"] = "esp_boot"
        out["result"] = list(loc["boot_reason"])
    elif vmt == "DeviceUUIDResponse":
        out["kind"] = "device_uuid_response"
        out["result"] = calls[0][1]
    elif vmt == "NVSResponse":
        if calls and calls[0][0] == "handleTareBehaviorNVSResponse":
            out["kind"] = "tare_nvs_response"
            out["result"] = calls[0][1]
        elif calls and calls[0][0] == "_handleNvsResponse":
            out["kind"] = "nvs_response"
            out["result"] = [calls[0][1], calls[0][2]]
        else:
            out["kind"] = "nvs_error"
            out["result"] = ",".join(loc["nvs_error"])
    elif vmt == "Notify":
        out["kind"] = "notify"
        out["result"] = [loc["notify"].notificationType, loc["notify"].message]
    elif vmt == "HeaterTimeoutInfo":
        out["kind"] = "heater"
        if "heater_timeout_info" in loc:
            out["result"] = render(loc["heater_timeout_info"])
            out["emits"] = enc(REC.emits)
        else:
            out["error"] = [msg for level, msg in REC.logs if level == "error"]
    elif vmt == "Log":
        out["kind"] = "log"
        errors = [
            msg
            for level, msg in REC.logs
            if level == "error" and msg.startswith("Error processing ESP log")
        ]
        if errors:
            out["error"] = errors
        else:
            out["result"] = {
                "level": loc["log_level"],
                "message": loc["message"],
                "full_message": loc["full_message"],
                "items": loc["items_filtered"],
                "malformed_items": [
                    msg[len("Error parsing ESP log item: ") :]
                    for level, msg in REC.logs
                    if level == "warning" and msg.startswith("Error parsing ESP log item: ")
                ],
                "send_to_sentry": loc["send_to_sentry"],
                "sentry": REC.sentry,
                "smoke": REC.smoke,
            }
    elif vmt == "MileageRequest":
        out["kind"] = "mileage_request"
    else:
        raise AssertionError(f"unexpected message type {vmt}")
    return out


BOOT = "rst:0x1 (POWERON_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)"
EDGE_LINES = [
    "CCW",
    "CW",
    "push",
    "pu_d",
    "elng",
    "ta_d",
    "ta_l",
    "strt",
    "tare",
    "ta_sl",
    "cntx",
    "  CCW  ",
    "CW,10",
    "Event,CCW,120",
    "Event,CW,9999+++",
    "Event,push,notanumber",
    "Event,ta_sl,42",
    "Event,cntx,7",
    "Event,tare_pressed,0",
    "Event,spare_1_pressed,0",
    "Event,encoder_button_released,05",
    "Event,unknown,1",
    "Event,doesnotexist,1",
    "Event,cw,1",
    "Event",
    "Event,",
    "Event,push,1,extra",
    "Data,0.00,0.00,0.00,S,21.93,idle,idle,none,0.00,none,0.00,false,0.00,none,0.00,none,0.00,Idle",
    "Data,1.5,0.8,36.2,S,92.1,brewing,My%20Profile,Pressure,9.0,Flow,2.0,true,1.2",
    "Data,1.5,0.8,36.2,U,92.1,brewing,Profile,none,0.0,none,0.0,false,0.4",
    "Data,1.5,0.8,36.2,S,92.1,idle,idle",
    "Data,0.0,0.0,-0.4,U,23.2,idle,Purge",
    "Data,nan,NaN,bogus,S,92.0",
    "Data,1,2,3,S,20",
    "Data,1,2,3,S",
    "Data,1,2,3",
    "Data,1,2",
    "Data,",
    "Data",
    "Data,1.5,0.8,36.2,S,92.1,heating,p,Pressure,bogus,Flow,2.0,true,1.2",
    "Data,1.5,0.8,36.2,S,92.1,heating,p,Pressure,9.0,Flow,bogus,true,1.2",
    "Data,1.5,0.8,36.2,S,92.1,heating,p,Flow,9.0,flow,3.0,true,nan",
    "Sensors,90.1,91.2,92.0,93.0,94.0,95.0,85.0,40.0,50.0,10.5,100.0,5.0,2.5,1.0,3.0,9.0,0.1,0.2,0.3,0.4,true,35.0,18.5,",
    "Sensors,90.1,91.2",
    "Sensors,5",
    "Sensors,",
    "Sensors",
    "Sensors,\x1b[1;31m ext1\x1b[0m90.1\x1b[1;32m ext2\x1b[0m91.2",
    "ESPInfo,0.9.10-sil,off,120,black,SIL-TM-0001,0025,2025-11-14,acaia,45.33,false,after_retraction,",
    "ESPInfo,1.0.0,1,24.0",
    "ESPInfo,2.0.0-dirty,1,24.0",
    "ESPInfo",
    "ESPInfo,",
    "ESPInfo,v1,2",
    "ESPBoot,PANIC,4",
    "ESPBoot,SW,3",
    "ESPBoot,,",
    "ESPBoot,SW",
    "ESPBoot,SW,3,extra",
    "device_uuid_response,SUCCESS",
    "device_uuid_response,accepted",
    "device_uuid_response,",
    "device_uuid_response",
    "device_uuid_response,a,b",
    "nvs_response,tare_behavior_key,SUCCESS",
    "nvs_response,tare_behavior_key,ERROR",
    "nvs_response,mileage_key,123",
    "nvs_response,mileage_key,ERROR: mileage seed not requested",
    "nvs_response,ERROR: key not found",
    "nvs_response",
    "nvs_response,color_key,a,b",
    "nvs_response,tare_behavior_key",
    "nvs_response,tare_behavior_key,a,b",
    "Notify,warning,Water tank empty",
    "Notify,warning,line one;line two",
    "Notify,info,a,b,c",
    "Notify,onlytype",
    "Notify,",
    "Notify",
    "Notify,acaia_msg,;",
    "HeaterTimeoutInfo,10.5,60.0,5.0,30.0",
    "HeaterTimeoutInfo,0,60.0,0,30.0",
    "HeaterTimeoutInfo,10.5,60.0,5.0",
    "HeaterTimeoutInfo,bogus,60.0,5.0,30.0",
    "HeaterTimeoutInfo",
    "HeaterTimeoutInfo,inf,nan,-0,1e3",
    "Log,ERROR,Heater fault detected",
    "Log,info,boot complete",
    "Log,INFO,no loadcell found",
    "Log,warning,low voltage,voltage=11.2,sentry=true",
    "Log,info,details,key=value=extra,malformed,k2=v2",
    "Log,info,details,sentry=false,a=1,a=2",
    "Log,debug,x,=empty,novalue=",
    "Log,info",
    "Log",
    "Log,",
    "Log,info,",
    "Log,Weird,msg,sentry=true,sentry=false",
    "Log,info,m,sentry=TRUE",
    "Log,info,m,,",
    "MileageRequest",
    "MileageRequest,",
    "mileagerequest",
    BOOT,
    BOOT + "\r\n",
    "\r" + BOOT,
    "rst:0x1 (POWERON_RESET),boot:0x13 (RTC_SW_SYS_RESET)",
    "Backtrace: 0x40081234:0x3ffb1234",
    "Guru Meditation Error: Core 1 panic'ed (LoadProhibited)",
    "ets Jul 29 2019 12:21:46",
    "random garbage line",
    "",
    ",",
    "\n",
    "\r\n",
    "Data\r\n",
    "Event,CW,1\r\r\n",
    "RECEIVED HASH: 0123456789abcdef",
    "MANUAL_CONTROL,flow,2",
    "CAL00",
    "ERROR: invalid action key",
]


def fuzz_lines(count):
    lines = []
    for _ in range(count):
        roll = RNG.random()
        if roll < 0.25:
            lines.append("Data," + ",".join(shot_args()))
        elif roll < 0.4:
            lines.append("Sensors," + ",".join(sensor_args()))
        elif roll < 0.55:
            lines.append("ESPInfo," + ",".join(esp_info_args()))
        elif roll < 0.65:
            args = button_args()
            lines.append(
                ("Event," + ",".join(args)) if RNG.random() < 0.8 else ",".join(args[:1])
            )
        elif roll < 0.72:
            lines.append("HeaterTimeoutInfo," + ",".join(heater_args()))
        elif roll < 0.85:
            items = [
                pick(
                    [
                        "a=1",
                        "sentry=true",
                        "sentry=false",
                        "k=v=w",
                        "bad",
                        "=x",
                        "y=",
                        "",
                        "a=2",
                    ]
                )
                for _ in range(RNG.choice([0, 1, 2, 3, 5]))
            ]
            parts = [
                pick(["info", "INFO", "Warning", "error", "ERROR", "debug", "verbose", ""]),
                pick(["msg", "", "Backtrace: x", "a;b"]),
            ] + items
            lines.append(
                "Log," + ",".join(parts[: RNG.choice([0, 1, 2, len(parts), len(parts)])])
            )
        elif roll < 0.92:
            n = RNG.choice([0, 1, 2, 3, 4])
            parts = [
                pick(
                    [
                        "tare_behavior_key",
                        "mileage_key",
                        "color_key",
                        "ERROR: x",
                        "SUCCESS",
                        "42",
                        "",
                    ]
                )
                for _ in range(n)
            ]
            lines.append(",".join(["nvs_response"] + parts))
        else:
            n = RNG.choice([0, 1, 2, 3])
            parts = [pick(["warning", "x;y", "", "acaia_msg", "a,b"]) for _ in range(n)]
            lines.append(",".join(["Notify"] + parts))
        if RNG.random() < 0.2:
            lines[-1] += pick(["\n", "\r\n", "\r"])
    return lines


def color_coded_lines(count):
    lines = []
    for _ in range(count):
        values = [pick(NUMBERS[:12]) for _ in range(20)] + [
            pick(BOOLS),
            pick(NUMBERS),
            pick(NUMBERS),
        ]
        text = "".join(
            f"\x1b[1;{pick(['31', '32', '33', '34', '35', '36'])}m {pick(['a0', 'ws', 'x_1'])}\x1b[0m{v}"
            for v in values
        )
        lines.append("Sensors," + text)
    return lines


def line_cases(emulated_lines):
    corpus = EDGE_LINES + emulated_lines[::40] + color_coded_lines(8) + fuzz_lines(900)
    cases = []
    for line in corpus:
        try:
            cases.append(dispatch_line(line))
        except BigInt:
            print(f"skipping big-int case: {line!r}")
    return cases


# --------------------------------------------------------------------------
# esp_observability.py
# --------------------------------------------------------------------------

OBS_LINES = [
    "rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)",
    "rst:0x3 (SW_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)\r\n",
    "\rrst:0x1 (POWERON_RESET),boot:0x8 (SPI_FAST_FLASH_BOOT)\n",
    "Guru Meditation Error: Core 1 panic'ed (LoadProhibited).",
    "Guru Meditation Error: Core 0 panic'ed (StoreProhibited). Exception was unhandled.",
    "guru meditation error:  core  1  PANIC'ED (Unhandled debug exception)",
    "Guru Meditation Error: Core 1 panic'ed",
    "Guru Meditation Error: something else",
    "Guru Meditation Error: Core 1 panic'ed (Task foo at 0x1234)",
    "Guru Meditation Error: Core 1 panic'ed (Interrupt wdt timeout on CPU1)",
    "abort() was called at PC 0x420037e1 on core 1",
    "ABORT() WAS CALLED AT PC 0x1 ON CORE 0",
    "abort() was called",
    "assert failed: x.cpp:42",
    "Stack smashing protect failure!",
    "Heap corruption detected",
    "Debug exception reason: Stack canary watchpoint triggered (Read ADC)",
    "Debug exception reason: Cache error (none) extra",
    "Debug exception reason: IllegalInstruction.",
    "Debug exception reason: foo (  )",
    "Debug exception reason:   ",
    "Debug exception reason: x ((y))",
    "Backtrace: 0x40381234:0x3fceabcd",
    "Core 1 register dump:",
    "Register dump:",
    "rst:garbled boot output",
    "random noise",
    "",
    "x" * 300,
    "é" * 150,
]
REASONS = ["PANIC", "SW", "TASK_WDT", " panic ", "", "brownout", "POWERON", "Int_WDT"]
CODES = ["4", "3", " 6 ", ""]
TYPES = ["Data", "Sensors", "Log", "ESPInfo", "ESPInfo", "ESPBoot", "Event", "NVSResponse"]
FIRMWARES = [None, "1.2.3", "2.0.0", " 2.0.0 ", "", "old", "new", "  "]


SNAPSHOT_FIELDS = [
    "phase",
    "update_in_progress",
    "last_valid_message",
    "recovery_deadline",
    "expected_firmware",
    "previous_firmware",
    "boot_seen",
    "pending_unexpected_reset",
    "reset_reported",
    "panic_reported",
    "timeout_reported",
]


def snapshot(m):
    """The public state, in SNAPSHOT_FIELDS order."""
    return [
        m.phase.value if name == "phase" else enc(getattr(m, name)) for name in SNAPSHOT_FIELDS
    ]


def diag(d):
    return {
        "title": d.title,
        "level": d.level,
        "fingerprint": d.fingerprint,
        "tags": enc(d.tags),
        "context": enc(d.context),
    }


def run_script(start, ops):
    monitor = ESPObservability(start)
    steps = []
    for op in ops:
        name, args = op[0], list(op[1:])
        method = getattr(monitor, name)
        result = method(*args)
        if isinstance(result, esp_observability.ESPDiagnostic):
            result = [result]
        steps.append(
            {
                "op": [name] + enc(args),
                "events": [diag(d) for d in (result or [])],
                "state": snapshot(monitor),
            }
        )
    return {"start": start, "fields": SNAPSHOT_FIELDS, "steps": steps}


def random_script():
    now = round(RNG.choice([0.0, 0.0, 5.0, 100.25]), 3)
    start = now
    ops = []
    for _ in range(RNG.randint(5, 45)):
        now = round(
            now
            + RNG.choice(
                [
                    0.0,
                    0.01,
                    0.1,
                    0.1,
                    0.3,
                    0.5,
                    0.51,
                    0.517,
                    0.0125,
                    1.0,
                    1.2345,
                    2.05,
                    5.0,
                    15.1,
                    30.5,
                    61.0,
                ]
            ),
            4,
        )
        roll = RNG.random()
        if roll < 0.35:
            ops.append(("observe_raw_line", pick(OBS_LINES), now))
        elif roll < 0.45:
            ops.append(("observe_boot_reason", pick(REASONS), pick(CODES), now))
        elif roll < 0.7:
            ops.append(("observe_valid_message", pick(TYPES), now, pick(FIRMWARES)))
        elif roll < 0.88:
            ops.append(("check_timeouts", now))
        elif roll < 0.92:
            ops.append(
                (
                    "begin_update",
                    pick([None, "2.0.0", "new", " "]),
                    pick([None, "1.2.3", "old", ""]),
                    now,
                )
            )
        elif roll < 0.95:
            ops.append(("finish_flashing", now))
        elif roll < 0.97:
            ops.append(
                (
                    "fail_flashing",
                    pick(["x" * 1100, "boom", "", "é" * 1001]),
                    pick(["flash", "exception"]),
                    now,
                )
            )
        else:
            ops.append(("begin_expected_reset", now))
    return start, ops


def observability_cases():
    scripts = []
    fixed = [
        (
            0.0,
            [
                (
                    "observe_raw_line",
                    "Guru Meditation Error: Core 1 panic'ed (LoadProhibited).",
                    1.0,
                )
            ]
            + [("observe_raw_line", f"register {i}", 1.1 + i / 100) for i in range(100)]
            + [
                ("observe_raw_line", OBS_LINES[0], 3.0),
                ("observe_boot_reason", "PANIC", "4", 4.0),
            ],
        ),
        (
            0.0,
            [("observe_raw_line", "abort() was called at PC 0x1 on core 0", 1.0)]
            + [("observe_raw_line", f"{i}:" + "é" * 700, 1.1 + i / 100) for i in range(20)]
            + [
                ("observe_raw_line", OBS_LINES[0], 3.0),
                ("observe_boot_reason", "PANIC", "4", 4.0),
            ],
        ),
        (
            0.0,
            [("observe_valid_message", "ESPInfo", 0.1, "1.2.3")]
            + [
                ("observe_raw_line", OBS_LINES[0], t)
                for t in (1.0, 10.0, 20.0, 30.0, 95.0, 100.0, 101.0)
            ]
            + [("check_timeouts", 0.6 + t) for t in (0, 100, 200)],
        ),
    ]
    # The byte bound cuts a multi-byte character at every alignment.
    for prefix in range(4):
        fixed.append(
            (
                0.0,
                [
                    ("observe_raw_line", "abort() was called at PC 0x1 on core 0", 1.0),
                    ("observe_raw_line", "x" * prefix, 1.01),
                ]
                + [("observe_raw_line", "é" * 500, 1.02 + i / 100) for i in range(12)]
                + [
                    ("observe_raw_line", OBS_LINES[0], 3.0),
                    ("observe_boot_reason", "PANIC", "4", 4.0),
                ],
            )
        )
    for start, ops in fixed:
        scripts.append(run_script(start, ops))
    for _ in range(120):
        scripts.append(run_script(*random_script()))
    return scripts


# --------------------------------------------------------------------------
# Machine._parseVersionString
# --------------------------------------------------------------------------

_parse_ns = {"version": packaging_version, "logger": RecordingLogger()}
exec(
    compile(
        segment(MACHINE_SRC, find_function(MACHINE_TREE, "_parseVersionString")),
        "machine.py:_parseVersionString",
        "exec",
    ),
    _parse_ns,
)
parse_version_string = _parse_ns["_parseVersionString"]

VERSIONS = [
    "1.2.3",
    "0.9.10-sil",
    "0.2.24-251-gd519a85",
    "1.2.3-dirty",
    "2.0.0-dirty",
    "2.0.0",
    "v1.2.3",
    "V1.2.3",
    "1.2.3rc1",
    "1.2.3.rc1",
    "1.2.3-rc1",
    "1.2",
    "1.2.0",
    "1.2.0.0",
    "1!1.0",
    "1.0.post1",
    "1.0post",
    "1.0.dev3",
    "1.0a",
    "1.0alpha2",
    "1.0b",
    "1.0beta1",
    "1.0c1",
    "1.0pre1",
    "1.0preview",
    "1.0.r1",
    "1.0rev2",
    "1.0+local.7",
    "1.0+Local_01-x",
    "01.002.0003",
    "1.0rc1.post2.dev3+abc",
    "1.0a1.dev1",
    "1.0.dev1",
    "1.0_post_1",
    "1.0.DEV",
    "1.0RC1",
    "\t1.0\t",
    " 1.2.3 ",
    "1.2.3 ",
    "1.0.0-0",
    "1.0.0-0-",
    "1.2.3--x",
    "1.2.3-4-g5-6-7",
    "1.2.3+ubuntu-1",
    "0.0.0",
    "0",
    "0.0",
    "99999999999999999999999.1",
    "99999999999999999999999.2",
    "1.0+abc.5",
    "1.0+abc.10",
    "1.0+5",
    "1.0+abc",
    "1.0.post0",
    "1.0-0",
    "1.0rc",
    "1.0rc0",
    "2!0.1",
    "1.0.0.1",
    "abc",
    "",
    " ",
    "1..2",
    "1.2.3+",
    "x1.2",
    "1.2.3-",
    "-1.2",
    "1.2.3.",
    "1.0+ab..c",
    "1.0-dev",
    "é1.0",
    None,
]


def version_cases():
    parsed = []
    for text in VERSIONS:
        result = parse_version_string(text)
        if result is None:
            parsed.append({"text": text, "result": None})
            continue
        parsed.append(
            {
                "text": text,
                "result": {
                    "release": str(result["Release"]),
                    "extra_commits": enc(result["ExtraCommits"]),
                    "sha": result["SHA"],
                    "local": result["Local"],
                },
            }
        )
    valid = [t for t in VERSIONS if t and parse_version_string(t) is not None]
    order = []
    equal = []
    for a in valid:
        va = parse_version_string(a)
        row_o = ""
        row_e = ""
        for b in valid:
            vb = parse_version_string(b)
            ra, rb = va["Release"], vb["Release"]
            row_o += "<" if ra < rb else ">" if ra > rb else "="
            row_e += "=" if va == vb else "!"
        order.append(row_o)
        equal.append(row_e)
    return {"parsed": parsed, "valid": valid, "release_order": order, "dict_equal": equal}


# --------------------------------------------------------------------------
# monitoring/motor_power_monitoring.py
# --------------------------------------------------------------------------


class Clock:
    wall = 1_700_000_000.0
    mono = 0.0


setattr(
    motor_module,
    "time",
    types.SimpleNamespace(time=lambda: Clock.wall, monotonic=lambda: Clock.mono),
)
POWERS = [0.0, 5.0, -3.5, 100.0, 0, 1e3, 12.25]
FLOWS = [2.5, 0.0, -1.0, 0, 0.004, float("inf"), "NaN", None, 7]
PRESSURES = [9.0, -2.0, "NaN", 0.0, 3]


def motor_script(window, ops):
    Clock.mono = 0.0
    calc = motor_module.EnergyCalculator(window, f"golden_{RNG.random()}")
    steps = []
    for op in ops:
        if op[0] == "calc":
            _, power, current, shot, now = op
            Clock.mono = now
            sensors = types.SimpleNamespace(motor_power=power, motor_current=current)
            shot_obj = (
                None if shot is None else types.SimpleNamespace(flow=shot[0], pressure=shot[1])
            )
            try:
                result = enc(calc.calculate_motor_energy(sensors, shot_obj))
            except TypeError:
                result = "TypeError"
        elif op[0] == "restart":
            calc.restart()
            result = None
        else:
            calc.set_window_time(op[1])
            result = None
        steps.append(
            {
                "op": enc(list(op)),
                "result": result,
                "total": enc(calc.total_energy),
                "history": len(calc.history),
                "window": enc(calc.window_seconds),
            }
        )
    return {"window": window, "steps": steps}


def motor_cases():
    scripts = []
    for _ in range(60):
        window = RNG.choice([600, 600, 1, 0, -5, 10])
        now = 0.0
        ops = []
        for _ in range(RNG.randint(3, 40)):
            roll = RNG.random()
            now = round(now + RNG.choice([0.1, 0.1, 0.05, -0.2, 0.0, 3.0, 300.0, 700.0]), 3)
            if roll < 0.85:
                shot = None if RNG.random() < 0.1 else (pick(FLOWS), pick(PRESSURES))
                ops.append(("calc", pick(POWERS), pick(POWERS), shot, now))
            elif roll < 0.92:
                ops.append(("set_window", pick([1, 5, 600, 0.5, 2.0])))
            else:
                ops.append(("restart",))
        scripts.append(motor_script(window, ops))

    restores = []
    for saved in ["1234.5", " 100\n", "abc", "", "inf", "nan", "-50", "1e5", "30000"]:
        for dt in [100.0, 0.0, -5.0, 1e6, 1.0]:
            for window in [600, 10]:
                path = os.path.join(_motor_dir, "motor_energy")
                with open(path, "w") as f:
                    f.write(saved)
                os.utime(path, (Clock.wall - dt, Clock.wall - dt))
                actual_dt = float(Clock.wall) - float(os.path.getmtime(path))
                Clock.mono = 1000.0
                calc = motor_module.EnergyCalculator(window, "motor")
                first = list(calc.history[0]) if calc.history else None
                last = list(calc.history[-1]) if calc.history else None
                total, history = calc.total_energy, len(calc.history)
                Clock.mono = 1000.1
                sensors = types.SimpleNamespace(motor_power=50.0, motor_current=2.0)
                after = calc.calculate_motor_energy(
                    sensors, types.SimpleNamespace(flow=2.0, pressure=9.0)
                )
                restores.append(
                    {
                        "saved": saved,
                        "dt": actual_dt,
                        "window": window,
                        "now": 1000.0,
                        "total": enc(total),
                        "history": history,
                        "first": enc(first),
                        "last": enc(last),
                        "after_calc_at_1000_1": enc(after),
                        "history_after_calc": len(calc.history),
                    }
                )
                os.remove(path)
    return {"scripts": scripts, "restores": restores}


# --------------------------------------------------------------------------
# UART writes of the real Machine methods
# --------------------------------------------------------------------------


def encode_cases():
    import machine as machine_module
    from machine import AlarmManager, Machine
    import heater_actuator

    class Port:
        def __init__(self):
            self.writes = []

        def write(self, data):
            self.writes.append(data.decode("utf-8"))

        def reset_input_buffer(self):
            self.writes.append("<reset_input_buffer>")

    port = Port()
    Machine._connection = types.SimpleNamespace(port=port)
    Machine._stopESPcomm = False
    AlarmManager.is_alarm_set = staticmethod(lambda _alarm: None)
    machine_module.NotificationManager.add_notification = staticmethod(lambda *_a, **_k: None)
    machine_module.MeticulousConfig.save = lambda *_a, **_k: None
    machine_module.ShotDebugManager._current_data = types.SimpleNamespace(nodeJSON=None)
    Machine.profileReady = True

    cases = []

    def record(name, args, fn):
        port.writes.clear()
        try:
            fn()
            result = list(port.writes)
        except ValueError as error:
            result = {"raises": str(error)}
        cases.append({"name": name, "args": enc(args), "writes": result})

    for action in [
        "info",
        "start",
        "stop",
        "tare",
        "home",
        "purge",
        "continue",
        "finish",
        "preheat",
        "scale_master_calibration",
        "a,b",
        "é",
    ]:
        record("action", [action], lambda: Machine.action(action))
    record("greeting", [], Machine._greet)
    for value in [45.33, 45, 0, 1e-5, 100.0, 12.345678901234567, -1.5, 1e16, "30.5"]:
        Machine.esp_info = None
        record("partial_retraction", [value], lambda: Machine.setPartialRetraction(value))
    for value in [True, False]:
        Machine.esp_info = None
        record("auto_purge", [value], lambda: Machine.setAutoPurgeAfterShot(value))
    for value in ["before_retraction", "after_retraction", "x,y", ""]:
        Machine.esp_info = ESPInfo(tareBehavior="other")
        Machine._pending_tare_behavior_writes = []
        record("tare_behavior", [value], lambda: Machine.setTareBehavior(value))
    record("mileage_read", [], Machine.requestMileage)
    for seed in [0, 1234, 999999999]:
        Machine._mileage_seed = seed
        record("mileage_seed", [seed], Machine._handleMileageRequest)
    record(
        "serial",
        ["black", "SN123", "0025", "2025-11-14"],
        lambda: Machine.setSerial("black", "SN123", "0025", "2025-11-14"),
    )
    for uuid in [UUIDS[0], "x"]:
        Machine.pending_device_uuid_assignment = uuid
        Machine.device_uuid_assignment_attempts = 0
        record("device_uuid_assign", [uuid], Machine._sendDeviceUUIDAssignment)
    for minutes in [0, 1, 30, 60, -1, 61]:
        record(
            "heater_timeout",
            [minutes],
            lambda: heater_actuator.HeaterActuator.set_timeout(minutes),
        )

    # backend.py's calibrate handler, cut out of backend.py.
    backend_src = (REPO / "backend.py").read_text()
    calibrate_ns = {}
    for weight in [0.0, 18.5, "NaN", 1e-5, -0.0, 100, float("inf")]:
        calibrate_ns["Machine"] = types.SimpleNamespace(
            data_sensors=types.SimpleNamespace(weight=weight), write=lambda b: port.write(b)
        )
        exec(
            compile(
                segment(backend_src, find_function(ast.parse(backend_src), "calibrate")),
                "backend.py:calibrate",
                "exec",
            ),
            calibrate_ns,
        )
        record("calibration", [weight], lambda: calibrate_ns["calibrate"](None))

    profiles = [json.loads((REPO / "simple_profile.json").read_text())]
    with open(REPO / "esp_serial/connection/emulated.shot.json") as f:
        profiles.append(json.load(f)["profile"])
    profiles += [
        {},
        [],
        {
            "name": "Café ☕",
            "n": 1,
            "f": 1.0,
            "e": 1e-7,
            "big": 1e16,
            "neg": -0.0,
            "l": [1, 2.5, None, True, False, 'a"b\\c\n'],
            "nested": {"x": {"y": []}},
        },
        {"emoji": "\U0001f600", "ctrl": "\x01\x1f\x7f", "keys": {"b": 1, "a": 2}},
    ]
    for profile in profiles:
        record("send_json_with_hash", [profile], lambda: Machine.send_json_with_hash(profile))
    return cases


# --------------------------------------------------------------------------
# ESPToolWrapper.get_version_from_firmware over synthetic images
# --------------------------------------------------------------------------


def app_desc(version=b"1.2.3", magic=0xABCD5432, project=b"fika"):
    desc = struct.pack("<II", magic, 0) + b"\0" * 8
    desc += version.ljust(32, b"\0")[:32] + project.ljust(32, b"\0")[:32]
    desc += b"12:00:00".ljust(16, b"\0") + b"Jan  1 2026".ljust(16, b"\0")
    desc += b"v5.1".ljust(32, b"\0") + b"\x11" * 32 + b"\0" * 80
    assert len(desc) == 256
    return desc


def image(segments, magic=0xE9, append_digest=1, count=None, checksum=True, digest=True):
    out = struct.pack(
        "<BBBBI", magic, len(segments) if count is None else count, 2, 0x2F, 0x40380000
    )
    out += (
        bytes([0xEE, 0, 0, 0])
        + struct.pack("<H", 9)
        + bytes([0])
        + struct.pack("<HH", 0, 0xFFFF)
    )
    out += bytes(4) + bytes([append_digest])
    for addr, data in segments:
        out += struct.pack("<II", addr, len(data)) + data
    if checksum:
        out += b"\0" * (15 - len(out) % 16) + b"\xef"
    if digest and append_digest == 1:
        out += b"\x22" * 32
    return out


DROM = 0x3C020020
IRAM = 0x40374000


def firmware_cases():
    import esp_serial.esp_tool_wrapper as wrapper

    images = {
        "valid": image([(DROM, app_desc() + b"rodata" * 10), (IRAM, b"\x90" * 64)]),
        "spaces_nul_tab": image([(DROM, app_desc(b" \t0.9.10-sil\0 \t"))]),
        "describe": image([(IRAM, b"\x01" * 16), (DROM, app_desc(b"0.2.24-251-gd519a85"))]),
        "no_digest": image([(DROM, app_desc())], append_digest=0),
        "digest_missing": image([(DROM, app_desc())], digest=False),
        "bad_magic": image([(DROM, app_desc())], magic=0xE8),
        "bad_append_digest": image([(DROM, app_desc())], append_digest=2),
        "truncated_segment": image([(DROM, app_desc())])[:200],
        "no_checksum": image([(DROM, app_desc())], checksum=False, digest=False),
        "no_drom": image([(IRAM, b"\x90" * 300)]),
        "short_drom": image([(DROM, b"\x01" * 100)]),
        "empty_drom": image([(DROM, b"")]),
        "wrong_desc_magic": image([(DROM, app_desc(magic=0x12345678))]),
        "two_droms_last_wins": image(
            [(DROM, app_desc(b"first")), (0x3C900000, app_desc(b"second"))]
        ),
        "last_drom_bad": image([(DROM, app_desc(b"first")), (0x3C900000, b"\x00" * 300)]),
        "extram_not_drom": image([(0x3D000000, app_desc(b"extram"))]),
        "too_many_segments": image([(IRAM, b"\x00" * 4)] * 16 + [(DROM, app_desc())]),
        "count_lies": image([(DROM, app_desc())], count=2),
        "bad_utf8_version": image([(DROM, app_desc(b"\xff\xfe"))]),
        "bad_utf8_project": image([(DROM, app_desc(project=b"\xc3"))]),
        "empty": b"",
    }
    cases = []
    with tempfile.TemporaryDirectory() as tmp:
        os.makedirs(os.path.join(tmp, "esp32-s3"))
        wrapper.UPDATE_PATH = tmp
        for name, data in images.items():
            with open(os.path.join(tmp, "esp32-s3", "firmware.bin"), "wb") as f:
                f.write(data)
            try:
                result = {"version": wrapper.ESPToolWrapper.get_version_from_firmware()}
            except Exception as error:  # noqa: BLE001
                result = {"raises": type(error).__name__}
            cases.append({"name": name, "image": data.hex(), **result})
    return cases


def compact(value):
    return json.dumps(value, separators=(",", ":"), ensure_ascii=True, allow_nan=False)


def lines_of(items):
    return "[\n" + ",\n".join(compact(item) for item in items) + "\n]"


def write(name, value):
    """One case per line: small files whose diffs show which cases changed."""
    path = OUT / f"{name}.json"
    if isinstance(value, list):
        text = lines_of(value)
    else:
        text = (
            "{\n"
            + ",\n".join(
                f"{compact(key)}:{lines_of(item) if isinstance(item, list) else compact(item)}"
                for key, item in value.items()
            )
            + "\n}"
        )
    path.write_text(text + "\n")
    print(f"wrote {path.relative_to(REPO)}", flush=True)


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    constructed, clones, emulated_lines = construct_cases()
    write("data", {"parsed": data_cases(), "constructed": constructed, "clones": clones})
    write("lines", line_cases(emulated_lines))
    write("observability", observability_cases())
    write("versions", version_cases())
    write("motor", motor_cases())
    write("encode", encode_cases())
    write("firmware_images", firmware_cases())


if __name__ == "__main__":
    main()
    # Be sure no thread an imported module started keeps the process alive.
    sys.stdout.flush()
    os._exit(0)
