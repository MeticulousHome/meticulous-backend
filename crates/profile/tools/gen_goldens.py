"""Generate the Python-side goldens the `profile` crate's tests compare against.

Every expected value comes from running the Python backend's own code
(`profile_preprocessor`, `profiles.ProfileManager`, `manual_program`,
`manual_mode`, `pour_over_profiles`, `jsonschema`) on a corpus of profiles and
on many mutated copies of them. Mutations are stored as edit operations on a
base document rather than as whole documents, which keeps the files small;
`tests/goldens.rs` applies the same operations.

`corpus/` holds every profile the old `default_profiles` submodule
(MeticulousHome/ProfileDefaults) shipped over its history, one file per
distinct blob, named after the blob id.

Run from the repository root, with the `profile_schema` and `log_redactor`
submodules checked out:

    uv sync --group dev
    uv run python crates/profile/tools/gen_goldens.py

Regenerate after a change to the Python code these goldens come from or to
the schema (copy `profile_schema/schema.json` to
`crates/profile/schemas/espresso_profile_schema.json` first).

The output is deterministic (fixed seeds, a counter instead of uuid4, UTC).
"""

import base64
import copy
import hashlib
import json
import logging
import os
import random
import sys
import tempfile
import time
import types
import uuid
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
DATA = HERE.parent / "tests" / "data"
CORPUS = HERE / "corpus"

# --- environment: what tests/conftest.py and the test modules set up -------

_TMP = tempfile.mkdtemp(prefix="meticulous-profile-goldens-")
for _name in (
    "CONFIG_PATH",
    "LOG_PATH",
    "HISTORY_PATH",
    "DEBUG_HISTORY_PATH",
    "ALARMS_PATH",
    "USER_SOUNDS",
    "MOTOR_ENERGY_PATH",
    "PROFILE_PATH",
    "IMAGES_PATH",
    "DEFAULT_IMAGES",
    "POUR_OVER_PROFILE_PATH",
):
    os.environ[_name] = os.path.join(_TMP, _name.lower())
os.environ["SMOKE_VALIDATION_STATE_FILE"] = os.path.join(_TMP, "smoke.json")
os.environ["REDACTION_KEY_PATH"] = os.path.join(_TMP, ".redaction_key")
os.environ["TZ"] = "UTC"
time.tzset()

sys.path.insert(0, str(REPO))
import tests.conftest  # noqa: E402,F401  (stubs gpiod/pydbus, keeps Sentry offline)

sys.modules.setdefault("pyprctl", types.SimpleNamespace(set_name=lambda _name: None))
sys.modules.setdefault(
    "monitoring.motor_power_monitoring",
    types.SimpleNamespace(
        motor_energy_calculator=types.SimpleNamespace(
            calculate_motor_energy=lambda *args, **kwargs: 0.0, total_energy=0.0
        ),
        MAX_ENERGY_ALLOWED=25000,
    ),
)

import jsonschema  # noqa: E402

import manual_mode  # noqa: E402
import pour_over_profiles  # noqa: E402
import profiles  # noqa: E402
from config import (  # noqa: E402
    CONFIG_USER,
    MACHINE_ALLOW_STAGE_SKIPPING,
    PROFILE_AUTO_PURGE,
    PROFILE_PARTIAL_RETRACTION,
    MeticulousConfig,
)
from pour_over_profiles import (  # noqa: E402
    PourOverProfileManager,
    PourOverProfileValidationError,
)
from profile_converter import simplified_json  # noqa: E402
from profile_preprocessor import ProfilePreprocessor  # noqa: E402
from profiles import ProfileManager  # noqa: E402

logging.disable(logging.CRITICAL)

ESPRESSO_SCHEMA = json.loads((REPO / "profile_schema" / "schema.json").read_text())
POUR_OVER_SCHEMA = json.loads((REPO / "pour_over_profile_schema" / "schema.json").read_text())
ProfileManager._schema = ESPRESSO_SCHEMA

# send_profile_to_esp32 minus its IO: what it hands to the UART is captured.
SENT = []
profiles.AlarmManager.is_alarm_set = staticmethod(lambda _alarm: None)
profiles.Machine.send_json_with_hash = staticmethod(SENT.append)
ProfileManager.handle_image = staticmethod(lambda _data: None)
ProfileManager._set_last_profile = staticmethod(lambda _profile: None)
ProfileManager._emit_profile_event = staticmethod(lambda *a, **k: None)
ProfileManager._async_emit_profile_hover = staticmethod(lambda *a, **k: None)
profiles.asyncio.run_coroutine_threadsafe = lambda *a, **k: None


def md5(text):
    return hashlib.md5(text.encode("utf-8")).hexdigest()


def write(name, payload):
    DATA.mkdir(parents=True, exist_ok=True)
    path = DATA / name
    path.write_text(json.dumps(payload, ensure_ascii=True, separators=(",", ":")) + "\n")
    print(f"wrote {path.relative_to(REPO)} ({path.stat().st_size // 1024} KiB)")


# --- edit operations ----------------------------------------------------------


def jpeg_data_uri(size):
    body = b"\xff\xd8" + b"\x00" * max(size - 4, 0) + b"\xff\xd9"
    return "data:image/jpeg;base64," + base64.b64encode(body).decode("ascii")


def apply_ops(doc, ops):
    doc = copy.deepcopy(doc)
    for op in ops:
        path = op["path"]
        if not path:
            assert op["op"] == "set"
            doc = copy.deepcopy(op["value"])
            continue
        parent = doc
        for step in path[:-1]:
            parent = parent[step]
        last = path[-1]
        kind = op["op"]
        if kind == "set":
            parent[last] = copy.deepcopy(op["value"])
        elif kind == "del":
            del parent[last]
        elif kind == "insert":
            parent.insert(last, copy.deepcopy(op["value"]))
        elif kind == "repeat":
            parent[last] = [copy.deepcopy(parent[last][0]) for _ in range(op["count"])]
        elif kind == "jpeg":
            parent[last] = jpeg_data_uri(op["size"])
        else:
            raise ValueError(kind)
    return doc


def get_path(doc, path):
    for step in path:
        doc = doc[step]
    return doc


def iter_paths(value, prefix=()):
    yield prefix
    if isinstance(value, dict):
        for key, child in value.items():
            yield from iter_paths(child, prefix + (key,))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from iter_paths(child, prefix + (index,))


COMMON_PALETTE = [
    None,
    True,
    False,
    0,
    1,
    -1,
    2,
    2.5,
    -0.5,
    0.0,
    1.0,
    99.99,
    100,
    100.0,
    101,
    150,
    1e10,
    2000,
    2000.0,
    2000.5,
    -100.5,
    -101,
    12345678901234,
    "",
    " ",
    "abc",
    "$",
    "$x",
    "$pressure_1",
    "$time_1",
    "x$",
    [],
    [1],
    [1, 2],
    [1, 2, 3],
    ["$a", 1],
    [[0, 1]],
    [[0, "$x"]],
    {},
    {"a": 1},
    {"type": "time", "value": 1},
    {"type": "flow"},
    "Caf\xe9 \U00002615",
    "\U0000200d\U0001f600",
    "\x00\x7f\x85\xa0",
    "a'b",
    'a"b',
    "a'\"b\\",
    "x\ny\tz",
    "#abcdef",
    "#ABCDEF\n",
    "#abcdeg",
    "#abc",
    "power",
    "flow",
    "pressure",
    "time",
    "weight",
    "piston_position",
    "user_interaction",
    "temperature",
    "linear",
    "none",
    "curve",
    ">=",
    "<=",
    "==",
    "11111111-2222-4333-8444-555555555555",
    "not-a-uuid",
    "/api/v1/profile/image/x.png",
    "x" * 101,
]


def random_op(rng, doc, palette, allow_root=False):
    paths = [p for p in iter_paths(doc) if p or allow_root]
    path = rng.choice(paths)
    if not path:
        return {"op": "set", "path": [], "value": rng.choice(palette)}
    parent = get_path(doc, path[:-1])
    target = get_path(doc, path)
    roll = rng.random()
    if roll < 0.25:
        return {"op": "del", "path": list(path)}
    if roll < 0.33 and isinstance(parent, list):
        return {"op": "insert", "path": list(path), "value": rng.choice(palette)}
    if roll < 0.40 and isinstance(target, dict):
        return {
            "op": "set",
            "path": list(path) + [f"extra_{rng.randrange(1000)}"],
            "value": rng.choice(palette),
        }
    if roll < 0.43 and isinstance(target, list) and target:
        return {"op": "repeat", "path": list(path), "count": rng.choice([0, 1, 3])}
    return {"op": "set", "path": list(path), "value": rng.choice(palette)}


def random_ops(rng, doc, palette, count):
    ops = []
    current = doc
    for _ in range(count):
        if not list(p for p in iter_paths(current) if p):
            break
        op = random_op(rng, current, palette)
        ops.append(op)
        current = apply_ops(current, [op])
    return ops


# --- base documents -------------------------------------------------------------


def corpus_profiles():
    out = []
    for path in sorted(CORPUS.glob("*.json")):
        out.append((path.stem, json.loads(path.read_text())))
    return out


def preprocessor_fixture():
    """tests/test_profile_preprocessor.py setUp."""
    return {
        "variables": [
            {"name": "Pressure", "key": "pressure_1", "type": "pressure", "value": 9},
            {"name": "Time", "key": "time_1", "type": "time", "value": 30},
        ],
        "stages": [
            {
                "name": "Test Stage",
                "type": "pressure",
                "dynamics": {
                    "points": [["$time_1", "$pressure_1"]],
                    "over": "time",
                    "interpolation": "linear",
                },
                "exit_triggers": [{"type": "time", "value": "$time_1"}],
                "limits": [
                    {"type": "flow", "value": 2.1},
                    {"type": "pressure", "value": "$pressure_1"},
                ],
            }
        ],
    }


def adaptive_profile(image_length=1000, metadata_length=100):
    """tests/test_profile_uart_payload.py, with a shorter embedded image."""
    return {
        "id": "a188c420-4952-4b33-8489-a082e9500ecb",
        "name": "Adaptive (Gagn\xe9 Method)",
        "author": "Corb3t",
        "author_id": "550e8400-e29b-41d4-a716-446655449999",
        "version": 9,
        "last_changed": 1776608894.8666232,
        "image": "data:image/png;base64," + "A" * image_length,
        "display": {
            "image": "/api/v1/profile/image/ae3e6ef14875bad88cad00f09945e681.png",
            "graphs": [{"x": "time", "y": "flow", "label": "Flow"}],
            "description": "Display-only recipe guidance",
        },
        "temperature": 90,
        "final_weight": 36,
        "variables": [
            {
                "key": "pressure_PeakLimit",
                "name": "Peak Pressure Limit",
                "type": "pressure",
                "value": 8.6,
            },
            {
                "key": "flow_TargetExtract",
                "name": "Target Extraction Flow",
                "type": "flow",
                "value": 4,
            },
            {
                "key": "pressure_PILimit",
                "name": "Pre-infusion Pressure Limit",
                "type": "pressure",
                "value": 4,
            },
            {
                "key": "time_FillPause",
                "name": "Fill Pause Duration",
                "type": "time",
                "value": 4,
            },
        ],
        "stages": [
            {
                "key": "fast_fill_1",
                "name": "Fast Fill",
                "type": "flow",
                "limits": [{"type": "pressure", "value": "$pressure_PILimit"}],
                "dynamics": {"over": "time", "points": [[0, 8]], "interpolation": "linear"},
                "exit_triggers": [
                    {
                        "type": "pressure",
                        "value": "$pressure_PILimit",
                        "relative": False,
                        "comparison": ">=",
                    }
                ],
            },
            {
                "key": "fill_pause_2",
                "name": "Fill Pause",
                "type": "flow",
                "limits": [{"type": "pressure", "value": "$pressure_PILimit"}],
                "dynamics": {
                    "over": "time",
                    "points": [[0, 0.5], ["$time_FillPause", 0.5]],
                    "interpolation": "linear",
                },
                "exit_triggers": [
                    {
                        "type": "time",
                        "value": "$time_FillPause",
                        "relative": True,
                        "comparison": ">=",
                    }
                ],
            },
            {
                "key": "pressure_ramp_3",
                "name": "Pressure Ramp",
                "type": "pressure",
                "limits": [{"type": "flow", "value": 6}],
                "dynamics": {
                    "over": "time",
                    "points": [[0, "$pressure_PILimit"], [4, "$pressure_PeakLimit"]],
                    "interpolation": "linear",
                },
                "exit_triggers": [
                    {"type": "time", "value": 4, "relative": True, "comparison": ">="}
                ],
            },
            {
                "key": "adaptive_extraction_4",
                "name": "Adaptive Extraction",
                "type": "flow",
                "limits": [{"type": "pressure", "value": "$pressure_PeakLimit"}],
                "dynamics": {
                    "over": "time",
                    "points": [[0, "$flow_TargetExtract"], [35, 0.5]],
                    "interpolation": "linear",
                },
                "exit_triggers": [
                    {"type": "time", "value": 45, "relative": True, "comparison": ">="}
                ],
            },
        ],
        "community": {"downloads": 123, "featured": True},
        "unknown_future_metadata": {"large": "B" * metadata_length},
    }


def v1_manual_profile():
    """tests/test_manual_mode.py: the v1 single-stage Manual mode profile."""
    return {
        "id": profiles.MANUAL_MODE_PROFILE_ID,
        "name": "My manual",
        "author": "Someone",
        "author_id": "11111111-1111-1111-1111-111111111111",
        "previous_authors": [],
        "temperature": 93.5,
        "final_weight": profiles.MANUAL_MODE_LEGACY_FINAL_WEIGHT_SENTINEL,
        "variables": [],
        "display": {"accentColor": "#abcdef"},
        "manual": True,
        "stages": [
            {
                "name": "Manual",
                "key": profiles.MANUAL_MODE_PRESSURE_STAGE_KEY,
                "type": "pressure",
                "dynamics": {"points": [[0, 0]], "over": "time", "interpolation": "none"},
                "exit_triggers": [{"type": "user_interaction", "value": 1}],
                "limits": [],
            }
        ],
    }


def espresso_bases():
    bases = [
        ("simple_profile", json.loads((REPO / "simple_profile.json").read_text())),
        (
            "example_profile",
            json.loads((REPO / "profile_schema" / "example_profile.json").read_text()),
        ),
        ("adaptive_profile", adaptive_profile()),
        ("preprocessor_fixture", preprocessor_fixture()),
        ("manual_mode", ProfileManager.build_manual_mode_profile()),
        ("manual_v1", v1_manual_profile()),
    ]
    bases.extend(corpus_profiles())
    return bases


# --- espresso validation ------------------------------------------------------


def validate_outcome(doc):
    """ProfileManager.validate_profile(doc): returned or raised."""
    try:
        err = ProfileManager.validate_profile(copy.deepcopy(doc))
    except Exception as raised:  # noqa: BLE001
        return {"kind": "other", "class": type(raised).__name__, "message": str(raised)}
    if err is None:
        return None
    if isinstance(err, jsonschema.exceptions.ValidationError):
        return {"kind": "schema", "message": err.message}
    return {
        "kind": "preprocess" if type(err).__name__ in VARIABLE_ERRORS else "other",
        "class": type(err).__name__,
        "message": str(err),
    }


VARIABLE_ERRORS = {"UndefinedVariableException", "VariableTypeException", "FormatException"}

DRAFT7 = jsonschema.Draft7Validator(ESPRESSO_SCHEMA)


def schema_outcome(doc):
    errors = list(DRAFT7.iter_errors(doc))
    best = jsonschema.exceptions.best_match(DRAFT7.iter_errors(doc))
    return {
        "errors": [[list(e.path), e.message, e.validator] for e in errors],
        "best": None if best is None else [list(best.path), best.message],
    }


def variable_ops(rng, doc):
    """Edits aimed at the variables: references, types, malformed lists."""
    ops = []
    variables = doc.get("variables") if isinstance(doc, dict) else None
    stages = doc.get("stages") if isinstance(doc, dict) else None
    keys = [v.get("key") for v in variables or [] if isinstance(v, dict)]
    if isinstance(stages, list):
        for s_index, stage in enumerate(stages):
            if not isinstance(stage, dict):
                continue
            points = (stage.get("dynamics") or {}).get("points")
            if isinstance(points, list):
                for p_index, point in enumerate(points[:2]):
                    if isinstance(point, list) and len(point) == 2:
                        for coord in (0, 1):
                            ref = "$" + str(rng.choice(keys)) if keys else "$missing"
                            ops.append(
                                [
                                    {
                                        "op": "set",
                                        "path": [
                                            "stages",
                                            s_index,
                                            "dynamics",
                                            "points",
                                            p_index,
                                            coord,
                                        ],
                                        "value": ref,
                                    }
                                ]
                            )
            for field in ("exit_triggers", "limits"):
                entries = stage.get(field)
                if isinstance(entries, list):
                    for e_index in range(min(len(entries), 2)):
                        ref = "$" + str(rng.choice(keys)) if keys else "$missing"
                        ops.append(
                            [
                                {
                                    "op": "set",
                                    "path": ["stages", s_index, field, e_index, "value"],
                                    "value": ref,
                                }
                            ]
                        )
                        ops.append(
                            [
                                {
                                    "op": "set",
                                    "path": ["stages", s_index, field, e_index, "type"],
                                    "value": rng.choice(
                                        ["time", "weight", "flow", "pressure", ["x"], 3]
                                    ),
                                }
                            ]
                        )
            ops.append(
                [
                    {
                        "op": "set",
                        "path": ["stages", s_index, "dynamics", "over"],
                        "value": rng.choice(
                            ["time", "weight", "type", "name", ["x"], {"a": 1}, 5]
                        ),
                    }
                ]
            )
            ops.append(
                [
                    {
                        "op": "set",
                        "path": ["stages", s_index, "time"],
                        "value": rng.choice(["pressure", "time", 3]),
                    }
                ]
            )
    if isinstance(variables, list):
        for v_index, var in enumerate(variables):
            ops.append(
                [
                    {
                        "op": "set",
                        "path": ["variables", v_index, "type"],
                        "value": rng.choice(
                            ["time", "flow", "pressure", "weight", 1, None, True]
                        ),
                    }
                ]
            )
            ops.append(
                [
                    {
                        "op": "set",
                        "path": ["variables", v_index, "key"],
                        "value": rng.choice([5, None, [1], {"a": 1}, "", "dup"]),
                    }
                ]
            )
            for field in ("key", "value", "type", "name"):
                ops.append([{"op": "del", "path": ["variables", v_index, field]}])
        ops.append(
            [
                {
                    "op": "insert",
                    "path": ["variables", 0],
                    "value": rng.choice(
                        [
                            5,
                            "ab",
                            None,
                            [1],
                            {"key": "k"},
                            {"key": [1], "value": 1, "type": "time"},
                        ]
                    ),
                }
            ]
        )
    for value in ({}, "ab", None, 5, {"k": {"key": "a"}}, [[1]], True):
        ops.append([{"op": "set", "path": ["variables"], "value": value}])
    return ops


def gen_validation():
    rng = random.Random(1)
    bases = espresso_bases()
    cases = []

    def add(base_index, ops):
        doc = apply_ops(bases[base_index][1], ops)
        cases.append(
            {
                "base": base_index,
                "ops": ops,
                "validate": validate_outcome(doc),
                "schema": schema_outcome(doc),
            }
        )

    for ops in targeted_example_ops():
        add(1, ops)

    for base_index, (_, doc) in enumerate(bases):
        add(base_index, [])
        exhaustive = base_index < 2
        if exhaustive:
            for path in [p for p in iter_paths(doc) if p]:
                add(base_index, [{"op": "del", "path": list(path)}])
                for value in rng.sample(COMMON_PALETTE, 6):
                    add(base_index, [{"op": "set", "path": list(path), "value": value}])
        for count, repeat in ((1, 20 if exhaustive else 50), (2, 15), (None, 8)):
            for _ in range(repeat):
                n = count if count is not None else rng.randint(3, 6)
                add(base_index, random_ops(rng, doc, COMMON_PALETTE, n))
        for ops in variable_ops(rng, doc):
            add(base_index, ops)
        for value in ([], "x", 5, None, True, {}, {"stages": "abc"}, {"stages": {"a": 1}}):
            add(base_index, [{"op": "set", "path": [], "value": value}])
    return {"bases": [doc for _, doc in bases], "names": [n for n, _ in bases], "cases": cases}


def targeted_example_ops():
    """Every keyword of the schema, at and around its bounds, on the example
    profile, alone and with sibling errors for best_match."""
    targeted = []
    for value in (
        1,
        2,
        2.0,
        1.5,
        0,
        0.5,
        -1,
        "1",
        True,
        None,
        1e300,
        12345678901234567,
        0.999999,
    ):
        targeted.append([{"op": "set", "path": ["version"], "value": value}])
    for field, low, high in (("temperature", 0, 100), ("final_weight", 0, 2000)):
        for value in (
            low,
            high,
            low - 1,
            high + 1,
            low - 0.1,
            high + 0.1,
            high + 1e-9,
            -0.0,
            float(high),
            "90",
            False,
        ):
            targeted.append([{"op": "set", "path": [field], "value": value}])
    for value in ("x" * 100, "x" * 101, "\xe9" * 100, "\xe9" * 101, "\U0001f600" * 101, "", 5):
        targeted.append(
            [{"op": "set", "path": ["display", "shortDescription"], "value": value}]
        )
    for value in (
        "#FF5733",
        "#ff5733",
        "#FF573",
        "#FF57333",
        "FF5733",
        "#GG5733",
        "#FF5733\n",
        "#FF5733\n\n",
        " #FF5733",
        "#\U00000661\U00000662\U00000663\U00000664\U00000665\U00000666",
        7,
    ):
        targeted.append([{"op": "set", "path": ["display", "accentColor"], "value": value}])
    for value in (-100, 100, -100.5, 100.5, "5", None):
        targeted.append(
            [{"op": "set", "path": ["stages", 0, "temperature_delta"], "value": value}]
        )
    for value in (
        [[1]],
        [[1, 2, 3]],
        [],
        [[]],
        [[0, "x"]],
        [["$", 1]],
        [[True, 1]],
        [[None, None]],
        [[{"a": 1}, [1]]],
        [5],
        ["ab"],
        {"a": 1},
        "ab",
    ):
        targeted.append(
            [{"op": "set", "path": ["stages", 0, "dynamics", "points"], "value": value}]
        )
    for field in ("name", "key", "type", "dynamics", "exit_triggers"):
        targeted.append([{"op": "del", "path": ["stages", 0, field]}])
    targeted.append(
        [
            {"op": "del", "path": ["stages", 0, "name"]},
            {"op": "del", "path": ["stages", 0, "key"]},
            {"op": "set", "path": ["stages", 0, "type"], "value": "x"},
        ]
    )
    targeted.append(
        [
            {"op": "del", "path": ["stages", 1, "name"]},
            {"op": "set", "path": ["stages", 0, "type"], "value": 5},
        ]
    )
    targeted.append(
        [
            {"op": "set", "path": ["temperature"], "value": 101},
            {"op": "set", "path": ["final_weight"], "value": -1},
            {"op": "del", "path": ["name"]},
        ]
    )
    targeted.append(
        [
            {
                "op": "set",
                "path": ["stages", 0, "exit_triggers", 0, "comparison"],
                "value": "==",
            },
            {
                "op": "set",
                "path": ["stages", 0, "exit_triggers", 1, "relative"],
                "value": "yes",
            },
        ]
    )
    targeted.append([{"op": "set", "path": ["previous_authors", 0], "value": {}}])
    targeted.append(
        [
            {"op": "set", "path": ["previous_authors", 1, "author_id"], "value": 5},
            {"op": "set", "path": ["previous_authors", 0, "name"], "value": None},
        ]
    )
    targeted.append(
        [
            {"op": "set", "path": ["variables", 0, "type"], "value": "temperature"},
            {"op": "set", "path": ["variables", 0, "value"], "value": "8"},
        ]
    )
    targeted.append(
        [{"op": "set", "path": ["stages", 1, "limits", 0, "type"], "value": "power"}]
    )
    targeted.append(
        [
            {"op": "set", "path": ["stages", 0, "dynamics", "over"], "value": "position"},
            {
                "op": "set",
                "path": ["stages", 0, "dynamics", "interpolation"],
                "value": "spline",
            },
        ]
    )
    return targeted


# --- processVariables directly ------------------------------------------------


def preprocess_outcome(doc):
    try:
        out = ProfilePreprocessor.processVariables(doc)
    except Exception as raised:  # noqa: BLE001
        return {"error": {"class": type(raised).__name__, "message": str(raised)}}
    text = json.dumps(out)
    return {"md5": md5(text), "len": len(text)}


STRUCTURE_PALETTE = COMMON_PALETTE + [
    "type dynamics points",
    "typevalue",
    ["type", "value"],
    ["type", "dynamics"],
    {"type": "time"},
    {"value": 1},
    {"0": 1},
    [[]],
    [[1]],
    [["$time_1"]],
    ["ab"],
    ["$x"],
    "$",
    {"points": [[1, 2]], "over": "time"},
    {"points": "ab", "over": "time"},
    {"points": {"a": 1}, "over": "time"},
    {"points": [[1, 2]]},
    ["points"],
    "points",
]


def gen_preprocessor():
    rng = random.Random(2)
    bases = espresso_bases()[:4]
    cases = []
    for base_index, (_, doc) in enumerate(bases):
        cases.append({"base": base_index, "ops": [], "expected": preprocess_outcome(doc)})
        for _ in range(250):
            ops = random_ops(rng, doc, STRUCTURE_PALETTE, rng.choice([1, 1, 1, 2, 3]))
            cases.append(
                {
                    "base": base_index,
                    "ops": ops,
                    "expected": preprocess_outcome(apply_ops(doc, ops)),
                }
            )
        for ops in variable_ops(rng, doc):
            cases.append(
                {
                    "base": base_index,
                    "ops": ops,
                    "expected": preprocess_outcome(apply_ops(doc, ops)),
                }
            )
    roots = [
        [],
        "x",
        5,
        None,
        True,
        {},
        {"stages": "abc"},
        {"stages": "type dynamics"},
        {"stages": {"a": 1}},
        {"stages": None},
        {"stages": 5},
        {"variables": {}},
        {"variables": "ab"},
        {"variables": None},
        {"variables": [{"key": "a", "value": 1}]},
        {"variables": [{"key": [1], "value": 1, "type": "t"}]},
        {"variables": [{"value": 1, "type": "t"}], "stages": 5},
        {
            "stages": [
                {
                    "type": "pressure",
                    "dynamics": {"points": [[0, 1]], "over": "time"},
                    "exit_triggers": None,
                }
            ]
        },
        {
            "stages": [
                {
                    "type": "pressure",
                    "dynamics": {"points": [[0, 1]], "over": "time"},
                    "limits": "ab",
                }
            ]
        },
        {"stages": [{"type": "pressure", "dynamics": {"points": [], "over": "time"}}]},
        {"stages": [{"type": "pressure", "dynamics": {"points": []}}]},
        {"stages": [{"type": "pressure", "dynamics": {"points": [[0, 1]]}}]},
        {
            "stages": [{"type": "pressure", "dynamics": {"points": "$a", "over": "time"}}],
            "variables": [{"key": "", "value": 1, "type": "time"}],
        },
        {
            "stages": [{"type": "pressure", "dynamics": {"points": ["$a"], "over": "time"}}],
            "variables": [{"key": "", "value": 1, "type": "time"}],
        },
        {
            "stages": [
                {
                    "type": "pressure",
                    "time": "flow",
                    "dynamics": {"points": [["$a", 1]], "over": "time"},
                }
            ],
            "variables": [{"key": "a", "value": 1, "type": "flow"}],
        },
        {
            "stages": [{"type": True, "dynamics": {"points": [[0, "$a"]], "over": "time"}}],
            "variables": [{"key": "a", "value": 1, "type": 1}],
        },
        {
            "stages": [{"type": 1.0, "dynamics": {"points": [[0, "$a"]], "over": "time"}}],
            "variables": [{"key": "a", "value": 1, "type": True}],
        },
        {
            "stages": [{"type": [1], "dynamics": {"points": [[0, "$a"]], "over": "time"}}],
            "variables": [{"key": "a", "value": 1, "type": [1.0]}],
        },
        {
            "stages": [
                {"type": {"a": None}, "dynamics": {"points": [[0, "$a"]], "over": "time"}}
            ],
            "variables": [{"key": "a", "value": 1, "type": {"a": None}}],
        },
        {
            "stages": [{"type": "x", "dynamics": {"points": [[0, "$a"]], "over": "time"}}],
            "variables": [
                {"key": "a", "value": 1, "type": "y"},
                {"key": "a", "value": 2, "type": "x"},
            ],
        },
        {
            "stages": [{"type": "x", "dynamics": {"points": [[0, "$\xe9"]], "over": "time"}}],
            "variables": [{"key": "\xe9", "value": [1, {"b": 2.5}], "type": "x"}],
        },
    ]
    root_cases = [{"input": doc, "expected": preprocess_outcome(doc)} for doc in roots]
    return {"bases": [doc for _, doc in bases], "cases": cases, "roots": root_cases}


# --- what reaches the ESP32 -----------------------------------------------------


def esp32_outcome(doc, settings, ref_start):
    MeticulousConfig[CONFIG_USER][PROFILE_AUTO_PURGE] = settings["auto_purge"]
    MeticulousConfig[CONFIG_USER][MACHINE_ALLOW_STAGE_SKIPPING] = settings[
        "allow_stage_skipping"
    ]
    MeticulousConfig[CONFIG_USER][PROFILE_PARTIAL_RETRACTION] = settings["partial_retraction"]
    simplified_json.current_reference_id = ref_start
    SENT.clear()
    data = copy.deepcopy(doc)
    if isinstance(data, dict) and "id" not in data:
        # The backend gives the profile an id before preparing it.
        data["id"] = "00000000-0000-4000-8000-00000000abcd"
    try:
        result = ProfileManager.send_profile_to_esp32(data)
    except Exception as raised:  # noqa: BLE001
        if isinstance(raised, jsonschema.exceptions.ValidationError):
            error = {"kind": "schema", "message": raised.message}
        elif type(raised).__name__ in VARIABLE_ERRORS:
            error = {
                "kind": "preprocess",
                "class": type(raised).__name__,
                "message": str(raised),
            }
        else:
            error = {"kind": "other", "class": type(raised).__name__, "message": str(raised)}
        return {"error": error, "ref_end": simplified_json.current_reference_id}
    assert result is data and len(SENT) == 1
    text = json.dumps(SENT[0])
    return {
        "md5": md5(text),
        "len": len(text),
        "ref_end": simplified_json.current_reference_id,
    }


SETTINGS = [
    {"auto_purge": a, "allow_stage_skipping": s, "partial_retraction": r}
    for a in (False, True)
    for s in (False, True)
    for r in (45.33, 36.26, 67.99, 50.0)
]
DEFAULT_SETTINGS = {
    "auto_purge": False,
    "allow_stage_skipping": False,
    "partial_retraction": 45.33,
}


def manual_variants():
    """Edit lists on the seeded Manual mode document."""
    stage_pressure = profiles.ProfileManager.manual_mode_stages()[0]
    stage_flow = profiles.ProfileManager.manual_mode_stages()[1]
    out = [
        [],
        [{"op": "set", "path": ["stages"], "value": [stage_flow, stage_pressure]}],
        [{"op": "set", "path": ["stages"], "value": [stage_pressure]}],
        [{"op": "set", "path": ["stages"], "value": [stage_flow]}],
        [
            {
                "op": "set",
                "path": ["stages"],
                "value": [stage_pressure, stage_flow, stage_pressure],
            }
        ],
        [
            {
                "op": "set",
                "path": ["stages"],
                "value": [stage_flow, stage_flow, stage_pressure, stage_flow, stage_pressure],
            }
        ],
        [{"op": "set", "path": ["stages"], "value": []}],
        [{"op": "set", "path": ["stages", 0, "type"], "value": "power"}],
        [{"op": "set", "path": ["stages", 1, "type"], "value": "power"}],
        [{"op": "set", "path": ["stages", 0, "dynamics", "points"], "value": []}],
        [{"op": "set", "path": ["stages", 1, "dynamics", "points"], "value": []}],
        [{"op": "set", "path": ["stages", 0, "dynamics", "points"], "value": [[0, 6.5]]}],
        [{"op": "set", "path": ["stages", 0, "dynamics", "points"], "value": [[0, 9]]}],
        [
            {
                "op": "set",
                "path": ["stages", 0, "dynamics", "points"],
                "value": [[0, 0.1], [5, 3]],
            }
        ],
        [
            {
                "op": "set",
                "path": ["stages", 0, "dynamics", "points"],
                "value": [[0, 12345678901234567]],
            }
        ],
        [
            {
                "op": "set",
                "path": ["variables"],
                "value": [{"name": "P", "key": "p", "type": "pressure", "value": 7.25}],
            },
            {"op": "set", "path": ["stages", 0, "dynamics", "points"], "value": [[0, "$p"]]},
        ],
        [
            {
                "op": "set",
                "path": ["variables"],
                "value": [{"name": "P", "key": "p", "type": "flow", "value": 7}],
            },
            {"op": "set", "path": ["stages", 0, "dynamics", "points"], "value": [[0, "$p"]]},
        ],
        [{"op": "set", "path": ["stages", 0, "name"], "value": "My pressure"}],
        [{"op": "set", "path": ["stages", 0, "name"], "value": ""}],
        [{"op": "set", "path": ["stages", 1, "name"], "value": "   "}],
        [{"op": "set", "path": ["stages", 1, "name"], "value": "\x1c\U00003000"}],
        [
            {
                "op": "set",
                "path": ["stages", 0, "name"],
                "value": "Caf\xe9 \U00002615 \U0000200d",
            }
        ],
        [{"op": "del", "path": ["stages", 0, "name"]}],
        [{"op": "set", "path": ["temperature"], "value": 90}],
        [{"op": "set", "path": ["temperature"], "value": 93.5}],
        [{"op": "set", "path": ["temperature"], "value": 0}],
        [{"op": "set", "path": ["temperature"], "value": 100}],
        [{"op": "set", "path": ["temperature"], "value": 99.99}],
        [{"op": "set", "path": ["temperature"], "value": 1.0000000000000002}],
        [{"op": "set", "path": ["final_weight"], "value": 36}],
        [{"op": "set", "path": ["final_weight"], "value": 42.5}],
        [{"op": "set", "path": ["final_weight"], "value": 0}],
        [{"op": "set", "path": ["final_weight"], "value": 1000.0}],
        [{"op": "set", "path": ["name"], "value": ""}],
        [{"op": "set", "path": ["name"], "value": "Caf\xe9 manual \U0001f600"}],
        [{"op": "del", "path": ["name"]}],
        [{"op": "del", "path": ["temperature"]}],
        [{"op": "set", "path": ["manual"], "value": "true"}],
        [{"op": "set", "path": ["manual"], "value": 1}],
        [{"op": "set", "path": ["manual"], "value": False}],
        [{"op": "del", "path": ["manual"]}],
        [{"op": "set", "path": ["id"], "value": "22222222-3333-4444-8555-666666666666"}],
        [{"op": "repeat", "path": ["stages"], "count": 40}],
        [{"op": "repeat", "path": ["stages"], "count": 3001}],
    ]
    return out


def gen_esp32():
    rng = random.Random(3)
    bases = espresso_bases()
    names = [n for n, _ in bases]
    manual_index = names.index("manual_mode")
    cases = []

    def add(base_index, ops, settings, ref_start, keep_payload=False):
        doc = apply_ops(bases[base_index][1], ops)
        outcome = esp32_outcome(doc, settings, ref_start)
        if keep_payload and "md5" in outcome:
            outcome["payload"] = json.dumps(SENT[0])
        cases.append(
            {
                "base": base_index,
                "ops": ops,
                "settings": settings,
                "ref_start": ref_start,
                "expected": outcome,
            }
        )

    for base_index, (name, doc) in enumerate(bases):
        add(base_index, [], DEFAULT_SETTINGS, 100, keep_payload=True)
        manual_doc = copy.deepcopy(doc)
        if isinstance(manual_doc, dict):
            add(
                base_index,
                [{"op": "set", "path": ["manual"], "value": True}],
                DEFAULT_SETTINGS,
                100,
            )
            add(
                base_index,
                [{"op": "set", "path": ["manual"], "value": True}],
                rng.choice(SETTINGS),
                rng.choice([100, 127, 4000]),
            )
        for _ in range(8):
            add(
                base_index,
                random_ops(rng, doc, COMMON_PALETTE, rng.choice([1, 1, 2])),
                DEFAULT_SETTINGS,
                100,
            )

    for settings in SETTINGS:
        for ref_start in (100, 103, 1000):
            add(manual_index, [], settings, ref_start, keep_payload=(ref_start == 100))
    for ops in manual_variants():
        add(manual_index, ops, DEFAULT_SETTINGS, 100, keep_payload=len(json.dumps(ops)) < 2000)
        add(manual_index, ops, rng.choice(SETTINGS), rng.choice([100, 109, 2500]))
    v1_index = names.index("manual_v1")
    for settings in SETTINGS[:4]:
        add(v1_index, [], settings, 100)
    return {"bases": [doc for _, doc in bases], "names": names, "cases": cases}


# --- manual shots ---------------------------------------------------------------


class Counter:
    def __init__(self):
        self.value = 0

    def __call__(self):
        self.value += 1
        return uuid.UUID(int=self.value)


def shot_sample(profile_time, setpoints, weight=0.0, use_profile_time=True):
    sample = {
        "shot": {
            "pressure": 6.0,
            "flow": 1.2,
            "weight": weight,
            "gravimetric_flow": 1.1,
            "setpoints": setpoints,
        },
        "time": profile_time,
        "status": "Manual",
    }
    if use_profile_time:
        sample["profile_time"] = profile_time
    return sample


def manual_shot(data, shot_time=1757193600.0, profile=None):
    if profile is None:
        profile = {
            "id": profiles.MANUAL_MODE_PROFILE_ID,
            "db_key": 7,
            "author": "Barista",
            "author_id": "11111111-2222-4333-8444-555555555555",
            "display": {},
            "final_weight": 1000.0,
            "last_changed": shot_time,
            "name": profiles.MANUAL_MODE_PROFILE_NAME,
            "temperature": 93.5,
            "stages": [],
            "variables": [],
            "previous_authors": [],
        }
    return {
        "id": "8f14e45f-ceea-467a-9a3c-a1b2c3d4e5f6",
        "db_key": 12,
        "time": shot_time,
        "file": "2026-09-07/10:00:00.shot.json.zst",
        "debug_file": None,
        "name": profiles.MANUAL_MODE_PROFILE_NAME,
        "data": data,
        "profile": profile,
    }


def random_shot(rng):
    data = []
    t = rng.choice([0, 0.0, 1000, 123.456])
    control = rng.choice(["pressure", "flow"])
    for _ in range(rng.randint(0, 30)):
        roll = rng.random()
        if roll < 0.2:
            control = "flow" if control == "pressure" else "pressure"
        step = rng.choice([0, 1, 7, 10, 33, 100, 250, 333.3, 1000, 1234.567, -50, 0.4, 5.0005])
        t = t + step
        target_kind = rng.random()
        if target_kind < 0.6:
            target = round(rng.uniform(0, 12), rng.choice([0, 1, 2, 3, 5]))
        elif target_kind < 0.75:
            target = rng.randint(0, 12)
        elif target_kind < 0.85:
            target = rng.choice([None, "6.0", True, [1], {"a": 1}])
        else:
            target = rng.uniform(0, 12)
        setpoints = {"active": control, control: target}
        if rng.random() < 0.08:
            setpoints = {"active": rng.choice([None, "temperature", "power", 5, ["pressure"]])}
        weight = rng.choice(
            [
                0.0,
                0.4,
                1.0,
                0.99,
                12.0,
                36.24,
                36.25,
                36.35,
                18,
                2,
                None,
                "36.2",
                True,
                rng.uniform(0, 50),
            ]
        )
        use_profile_time = rng.random() < 0.8
        sample = shot_sample(t, setpoints, weight=weight, use_profile_time=use_profile_time)
        if rng.random() < 0.05:
            sample["profile_time"] = None
        if rng.random() < 0.03:
            sample = rng.choice(
                ["not a sample", {"shot": "x"}, {"shot": {"setpoints": None}}, 5, None]
            )
        data.append(sample)
    profile = manual_shot([])["profile"]
    if rng.random() < 0.3:
        profile["temperature"] = rng.choice([None, "93.5", 90, 88.8, True, [1]])
    if rng.random() < 0.2:
        profile["author"] = rng.choice([None, "", 0, "Someone", ["x"]])
    if rng.random() < 0.2:
        profile["author_id"] = rng.choice([None, "", "abc"])
    shot = manual_shot(
        data,
        shot_time=rng.choice([1757193600.0, 1757193659.9, 0, 86399.5, 1700000000]),
        profile=profile,
    )
    if rng.random() < 0.05:
        shot["profile"] = rng.choice([None, {}, []])
    return shot


def manual_shot_outcome(shot, name):
    counter = Counter()
    original = uuid.uuid4
    uuid.uuid4 = counter
    try:
        profile = manual_mode.build_profile_from_manual_shot(copy.deepcopy(shot), name)
    except manual_mode.ManualShotHasNoTargets:
        return {"error": {"class": "ManualShotHasNoTargets"}}
    except Exception as raised:  # noqa: BLE001
        return {"error": {"class": type(raised).__name__, "message": str(raised)}}
    finally:
        uuid.uuid4 = original
    return {"profile": profile, "uuids": counter.value}


def gen_manual_shots():
    rng = random.Random(4)

    def p(t, v, **kw):
        return shot_sample(t, {"active": "pressure", "pressure": v}, **kw)

    def f(t, v, **kw):
        return shot_sample(t, {"active": "flow", "flow": v}, **kw)

    mixed = manual_shot(
        [
            shot_sample(0, {"active": "temperature", "temperature": 93.5}),
            p(1000, 6.0, weight=0.0),
            p(2000, 8.456, weight=12.0),
            p(2000, 9.0, weight=13.0),
            shot_sample(3000, {"active": None}),
            shot_sample(3100, {"active": "pressure"}),
            p(3500, 4.0, weight=36.24),
        ]
    )
    switching = manual_shot(
        [
            p(0, 6.0, weight=0.0),
            p(1000, 7.0, weight=2.0),
            f(2000, 2.5, weight=8.0),
            f(3000, 2.0, weight=14.0),
            p(4000, 4.0, weight=20.0),
        ]
    )
    shots = [
        (mixed, None),
        (mixed, ""),
        (mixed, "   "),
        (mixed, "   Morning manual   "),
        (mixed, "m" * 100),
        (mixed, "e\U00000301" * 40),
        (mixed, "\x1c Name \U00003000"),
        (switching, None),
        (
            manual_shot(
                [p(4000, 5.0, use_profile_time=False), p(6250, 7.0, use_profile_time=False)]
            ),
            None,
        ),
        (
            manual_shot(
                [p(1000, 6.0, weight=5.0), p(2000, 7.0, weight=30.0), p(2000, 7.0, weight=31.5)]
            ),
            None,
        ),
        (manual_shot([p(1000, 6.0, weight=0.4)]), None),
        (manual_shot([p(1000, 6.0, weight=None)]), None),
        (manual_shot([p(1000, 6.0, weight="36.2")]), None),
        (manual_shot([p(1000, 6.0)]), None),
        (manual_shot([f(0, 2.0), f(1000, 2.4, weight=18.0)]), None),
        (
            manual_shot(
                [
                    p(0, 6.0),
                    shot_sample(1000, {"active": None}),
                    shot_sample(1500, {"active": "pressure"}),
                    p(2000, 7.0),
                ]
            ),
            None,
        ),
        (manual_shot([p(0, 6.0), f(1000, 2.0, weight=0.2)]), None),
        (manual_shot([p(0, 6.0), f(500, 2.0), p(1000, 7.0), f(1500, 2.4)]), None),
        (manual_shot([]), None),
        (manual_shot(None), None),
        (manual_shot([shot_sample(0, {"active": "temperature", "temperature": 93.5})]), None),
        (
            manual_shot(
                ["not a sample", {"shot": "not a dict"}, {"shot": {"setpoints": None}}]
            ),
            None,
        ),
        (manual_shot(5), None),
        (manual_shot(True), None),
        (manual_shot("abc"), None),
        (manual_shot({"a": 1}), None),
        (manual_shot([p(1000, 6.0, weight=18)]), None),
        (manual_shot([p(1000, 6, weight=36.25)]), None),
        (manual_shot([p(1000, 6.125), p(1125, 6.135), p(1130, 6.145)]), None),
    ]
    no_time = manual_shot([p(1000, 6.0)])
    del no_time["time"]
    shots.append((no_time, None))
    shots.append((no_time, "named"))
    for author in (None, "", 0, ["x"]):
        shot = manual_shot([p(1000, 6.0)])
        shot["profile"]["author"] = author
        shot["profile"]["author_id"] = author
        shots.append((shot, None))
    for temperature in (None, "93.5", 90, True):
        shot = manual_shot([p(1000, 6.0)])
        shot["profile"]["temperature"] = temperature
        shots.append((shot, None))
    for _ in range(250):
        shots.append((random_shot(rng), rng.choice([None, None, "", "Shot", "  x  "])))
    return {
        "cases": [
            {"shot": shot, "name": name, "expected": manual_shot_outcome(shot, name)}
            for shot, name in shots
        ]
    }


# --- profiles.py helpers --------------------------------------------------------


def manual_mode_known_profiles():
    """The known Manual mode profiles ensure_manual_mode_profile meets
    (tests/test_manual_mode.py's cases and a few more)."""
    pid = profiles.MANUAL_MODE_PROFILE_ID
    stages = ProfileManager.manual_mode_stages()
    pressure = {"key": "manual_pressure", "type": "pressure"}
    flow = {"key": "manual_flow", "type": "flow"}
    return [
        None,
        {"id": pid, "stages": stages},
        {"id": pid, "stages": [stages[1], stages[0]]},
        v1_manual_profile(),
        {"id": pid, "final_weight": 36.0, "stages": []},
        {"id": pid, "manual": False, "final_weight": 1000, "stages": [], "name": "x"},
        {"id": pid, "stages": []},
        {"id": pid, "stages": None},
        {"id": pid, "stages": "not a list"},
        {"id": pid, "stages": [pressure]},
        {"id": pid, "stages": [dict(pressure, type="flow"), dict(flow, type="pressure")]},
        {"id": pid, "stages": [pressure, pressure]},
        {"id": pid, "stages": [pressure, flow, {"key": "extra", "type": "flow"}]},
        {"id": pid, "stages": [{"type": "pressure"}, flow]},
    ]


def gen_profiles_helpers():
    seeds = []
    for existing in manual_mode_known_profiles():
        written = []
        ProfileManager._known_profiles = (
            {}
            if existing is None
            else {profiles.MANUAL_MODE_PROFILE_ID: copy.deepcopy(existing)}
        )
        original_save = ProfileManager.save_profile
        ProfileManager.save_profile = lambda data, **kwargs: written.append(
            copy.deepcopy(data)
        ) or {"profile": data}
        try:
            result = ProfileManager.ensure_manual_mode_profile()
        finally:
            ProfileManager.save_profile = original_save
        seeds.append(
            {"existing": existing, "written": written[0] if written else None, "result": result}
        )

    normalize = []
    docs = [
        {
            "id": "a",
            "author": "x",
            "author_id": "y",
            "last_changed": 1.0,
            "stages": [{"key": "k"}],
        },
        {"name": "no id"},
        {"id": "", "author_id": "", "stages": [{"name": "a"}, {"key": "b"}, {}]},
        {"id": None, "author_id": None, "last_changed": None, "stages": []},
        {"id": "x", "author": "a", "author_id": "b", "last_changed": 2, "stages": {"a": 1}},
        {"id": "x", "author": "a", "author_id": "b", "last_changed": 2, "stages": "key"},
        {"id": "x", "author": "a", "author_id": "b", "last_changed": 2, "stages": "abc"},
        {"id": "x", "author": "a", "author_id": "b", "last_changed": 2, "stages": [5]},
        {"id": "x", "author": "a", "author_id": "b", "last_changed": 2, "stages": None},
        {
            "id": "x",
            "author": "a",
            "author_id": "b",
            "last_changed": 2,
            "stages": [["key"], ["x"]],
        },
        [],
        ["id"],
        "id",
        "abc",
        5,
        None,
    ]
    for doc in docs:
        counter = Counter()
        work = copy.deepcopy(doc)
        try:
            changed = refresh_normalisation(work, counter)
            normalize.append(
                {"input": doc, "output": work, "changed": changed, "uuids": counter.value}
            )
        except Exception as raised:  # noqa: BLE001
            normalize.append(
                {
                    "input": doc,
                    "error": {"class": type(raised).__name__, "message": str(raised)},
                }
            )
    return {
        "manual_mode_profile": ProfileManager.build_manual_mode_profile(),
        "manual_mode_stages": ProfileManager.manual_mode_stages(),
        "seeds": seeds,
        "normalize": normalize,
    }


def refresh_normalisation(profile, counter):
    """The body of ProfileManager.refresh_profile_list for one decoded file,
    up to the validation (copied from profiles.py, which inlines it)."""
    profile_changed = False
    if "id" not in profile or profile["id"] == "":
        profile["id"] = str(counter())
        profile_changed = True
    if "author" not in profile:
        profile["author"] = ""
        profile_changed = True
    if "author_id" not in profile or profile["author_id"] == "":
        profile["author_id"] = str("00000000-0000-0000-0000-000000000000")
        profile_changed = True
    if "last_changed" not in profile:
        profile_changed = True
    if "stages" not in profile:
        profile["stages"] = []
        profile_changed = True
    else:
        for stage in profile["stages"]:
            if "key" not in stage:
                stage["key"] = str(counter())
                profile_changed = True
    return profile_changed


# --- Pour Over ------------------------------------------------------------------


def valid_pour_over_profile():
    """tests/test_pour_over_profiles.py."""
    return {
        "version": 1,
        "brew_type": "pour_over",
        "id": "11111111-2222-4333-8444-555555555555",
        "name": "Lance 2 pour",
        "author": "Recipe author",
        "author_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
        "display": {
            "accentColor": "#23383F",
            "shortDescription": "A bright three-stage recipe",
            "description": "Two smaller pours followed by the main pour.",
        },
        "recipe": {
            "coffee_dose_g": 15,
            "total_water_g": 225,
            "water_temperature_c": 92,
            "target_total_time_s": 135,
            "target_total_time_max_s": 180,
        },
        "stages": [
            {
                "key": "pour-1",
                "name": "Pour 1",
                "starts_at_s": 0,
                "pour": {
                    "water_g": 35,
                    "duration_s": 10,
                    "target_cumulative_water_g": 35,
                    "flow_rate_g_s": 3.5,
                    "pattern": "spiral_out",
                },
            },
            {
                "key": "pour-2",
                "name": "Pour 2",
                "starts_at_s": 30,
                "pour": {
                    "water_g": 35,
                    "duration_s": 10,
                    "target_cumulative_water_g": 70,
                    "flow_rate_g_s": 3.5,
                    "flow_range_g_s": [3, 4],
                },
            },
            {
                "key": "pour-3",
                "name": "Pour 3",
                "starts_at_s": 60,
                "pour": {
                    "water_g": 155,
                    "duration_s": 31,
                    "target_cumulative_water_g": 225,
                    "flow_rate_g_s": 5,
                },
            },
        ],
    }


POUR_OVER_PALETTE = COMMON_PALETTE + [
    "pour_over",
    "espresso",
    4.9,
    5,
    40,
    40.1,
    69,
    70,
    100.5,
    600,
    600.5,
    2001,
    0.001,
    3.4,
    3.6,
    "x" * 80,
    "x" * 81,
    "x" * 120,
    "x" * 121,
    "x" * 200,
    "x" * 201,
    "x" * 300,
    "x" * 301,
    "x" * 500,
    "x" * 501,
    "   ",
    "center",
    "spiral",
    "clockwise",
    "{11111111-2222-4333-8444-555555555555}",
    "urn:uuid:11111111-2222-4333-8444-555555555555",
    "1111_111-2222-4333-8444-555555555555",
    "11111111222243338444555555555555",
    "data:image/jpeg;base64,/9j/2Q==",
    "data:image/jpeg;base64,/9j/2Q=",
    "data:image/jpeg;base64,/9j/2Q",
    "data:image/jpeg;base64,iVBORw0KGgo=",
    "data:image/jpeg;base64,/9j/\xe9Q==",
    "data:image/jpeg;base64,/9j/ 2Q==",
    "data:image/jpeg;base64,",
    "data:image/png;base64,/9j/2Q==",
    [3, 4],
    [4, 3],
    [1, 2],
    [3, 4, 5],
    [-1, 4],
    [3, 0],
]


def pour_over_outcome(doc):
    try:
        PourOverProfileManager.validate_profile(copy.deepcopy(doc))
    except PourOverProfileValidationError as error:
        return {"issues": error.issues}
    text = json.dumps(doc, ensure_ascii=False, allow_nan=False, indent=2)
    return {"ok": True, "md5": md5(text), "bytes": len(text.encode("utf-8"))}


def gen_pour_over():
    rng = random.Random(6)
    PourOverProfileManager._ensure_validator()
    example = json.loads(
        (REPO / "pour_over_profile_schema" / "example_profile.json").read_text()
    )
    with_image = valid_pour_over_profile()
    with_image["display"]["image"] = "data:image/jpeg;base64,/9j/2Q=="
    bases = [
        ("valid", valid_pour_over_profile()),
        ("example", example),
        ("with_image", with_image),
    ]
    cases = []

    def add(base_index, ops):
        doc = apply_ops(bases[base_index][1], ops)
        cases.append({"base": base_index, "ops": ops, "expected": pour_over_outcome(doc)})

    for base_index, (_, doc) in enumerate(bases):
        add(base_index, [])
        for path in [p for p in iter_paths(doc) if p]:
            add(base_index, [{"op": "del", "path": list(path)}])
            for value in rng.sample(POUR_OVER_PALETTE, 5):
                add(base_index, [{"op": "set", "path": list(path), "value": value}])
        for _ in range(60):
            add(base_index, random_ops(rng, doc, POUR_OVER_PALETTE, rng.choice([1, 2, 3])))
        # Semantic edits: small numeric perturbations that stay schema-valid.
        numeric_paths = [
            p
            for p in iter_paths(doc)
            if p
            and isinstance(get_path(doc, p), (int, float))
            and not isinstance(get_path(doc, p), bool)
        ]
        for _ in range(120):
            ops = []
            for path in rng.sample(numeric_paths, rng.choice([1, 1, 2])):
                value = get_path(doc, path)
                delta = rng.choice(
                    [0.05, 0.1, 0.11, 0.5, 1, 2, 5, 10, -0.05, -0.1, -1, -5, 0.001, 0.0005]
                )
                new = value + delta
                if rng.random() < 0.3:
                    new = round(new, rng.choice([0, 1, 2]))
                ops.append({"op": "set", "path": list(path), "value": new})
            add(base_index, ops)
    image_index = 2
    for size in (1, 3, 4, 5, 100, 300 * 1024, 300 * 1024 + 1, 307_000):
        add(image_index, [{"op": "jpeg", "path": ["display", "image"], "size": size}])
    for image in (
        "data:image/jpeg;base64,iVBORw0KGgo=",
        "data:image/jpeg;base64,/9j/",
        "data:image/jpeg;base64,/9g/2Q==",
        "data:image/jpeg;base64,/9j/2A==",
        "data:image/jpeg;base64,/9k=",
        "data:image/jpeg;base64,",
        "data:image/jpeg;base64,/9j/2Q=",
        "data:image/jpeg;base64,=/9j/2Q==",
        "data:image/jpeg;base64,/9j/2Q==/9j/",
        "data:image/jpeg;base64,/9j/\xe92Q==",
        "data:image/jpeg;base64,/9j/2Q==\n",
        "data:image/jpeg;base64,/9j/2Q",
        "data:image/jpeg;base64,/9j/AAAA2Q==",
    ):
        add(image_index, [{"op": "set", "path": ["display", "image"], "value": image}])
    add(image_index, [{"op": "set", "path": ["stages", 1, "key"], "value": "pour-1"}])
    add(
        image_index,
        [
            {"op": "set", "path": ["stages", 0, "key"], "value": " "},
            {"op": "set", "path": ["name"], "value": "\x1c"},
        ],
    )
    roots = [[], "x", 5, None, True, {}]
    root_cases = [{"input": doc, "expected": pour_over_outcome_any(doc)} for doc in roots]

    bodies = []
    limit = pour_over_profiles.MAX_POUR_OVER_PROFILE_BYTES
    pattern = __import__(
        "api.profiles", fromlist=["POUR_OVER_BREW_TYPE_PATTERN"]
    ).POUR_OVER_BREW_TYPE_PATTERN
    for prefix, suffix in (
        ('{"brew_type":"pour_over","padding":"', '"}'),
        ('{"brew_type" :\n\t"pour_over","padding":"', '"}'),
        ('{"brew_type":"espresso","padding":"', '"}'),
        ('{"padding":"', '","brew_type": "pour_over"}'),
        ('{"brew_type":\xa0"pour_over","padding":"', '"}'),
    ):
        for pad in (limit - 100, limit, limit + 100):
            body = (prefix + "x" * pad + suffix).encode("utf-8")
            bodies.append(
                {
                    "prefix": prefix,
                    "pad": pad,
                    "suffix": suffix,
                    "expected": bool(len(body) > limit and pattern.search(body)),
                }
            )
    return {
        "bases": [doc for _, doc in bases],
        "cases": cases,
        "roots": root_cases,
        "bodies": bodies,
    }


def pour_over_outcome_any(doc):
    try:
        PourOverProfileManager.validate_profile(copy.deepcopy(doc))
    except PourOverProfileValidationError as error:
        return {"issues": error.issues}
    return {"ok": True}


# --- CPython primitives ---------------------------------------------------------


def random_string(rng):
    pools = [
        lambda: chr(rng.randrange(0x20, 0x7F)),
        lambda: chr(rng.randrange(0, 0x20)),
        lambda: chr(rng.randrange(0x7F, 0x100)),
        lambda: chr(rng.randrange(0x100, 0x3000)),
        lambda: chr(
            rng.choice(
                [
                    0x200B,
                    0x200D,
                    0x2028,
                    0x2029,
                    0xFEFF,
                    0x00AD,
                    0x061C,
                    0xE000,
                    0xFFFF,
                    0x0378,
                    0x1680,
                    0x3000,
                    0x180E,
                ]
            )
        ),
        lambda: chr(rng.randrange(0x10000, 0x110000)),
        lambda: chr(rng.randrange(0xE000, 0x10000)),
        lambda: rng.choice(["'", '"', "\\", "\n", "\t", "\r"]),
    ]
    out = []
    for _ in range(rng.randint(0, 8)):
        cp = rng.choice(pools)()
        if 0xD800 <= ord(cp) <= 0xDFFF:
            cp = "?"
        out.append(cp)
    return "".join(out)


def gen_primitives():
    rng = random.Random(7)
    strings = [random_string(rng) for _ in range(4000)]
    strings += [
        "",
        "'",
        '"',
        "'\"",
        "\\",
        "a'b",
        'a"b',
        "\x1c\x1d\x1e\x1f",
        " \t\n\x0b\x0c\r\x85\xa0\U00001680\U00002000\U0000200a\U00002028\U00002029\U0000202f\U0000205f\U00003000\U0000200b\U0000180e",
    ]
    repr_cases = [[s, repr(s), s.strip(), s.isprintable()] for s in strings]
    values = [
        None,
        True,
        [1, 2.5, None, "x", {"a": [True, False]}],
        {"a'b": 'c"d', "": {}},
        1e16,
        1e-05,
        -0.0,
        123456789012345678,
        -9223372036854775808,
        18446744073709551615,
        0.1 + 0.2,
        [[0, "$x"]],
    ]
    value_cases = [[v, repr(v)] for v in values]

    floats = []
    for _ in range(3000):
        kind = rng.random()
        if kind < 0.3:
            x = rng.randint(-100000, 100000) / 1000
        elif kind < 0.5:
            x = rng.randint(-10000, 10000) / 200 + rng.choice([0, 0.005, -0.005, 0.0005])
        elif kind < 0.7:
            x = rng.uniform(-1000, 1000)
        elif kind < 0.85:
            x = rng.uniform(-1, 1) * 10 ** rng.randint(-8, 20)
        else:
            x = rng.choice(
                [
                    0.125,
                    0.375,
                    2.675,
                    8.456,
                    36.24,
                    36.25,
                    36.35,
                    0.05,
                    0.15,
                    0.25,
                    1.005,
                    2.5,
                    3.5,
                    -0.001,
                    -0.0,
                    0.0,
                    1e300,
                    5e-324,
                    123456789.125,
                ]
            )
        floats.append([x, round(x, 2), round(x, 1), f"{x:.2f}", f"{x:.1f}"])

    b64 = []
    for _ in range(1500):
        raw = bytes(rng.randrange(256) for _ in range(rng.randint(0, 12)))
        text = base64.b64encode(raw).decode("ascii")
        roll = rng.random()
        if roll < 0.4:
            pos = rng.randint(0, len(text))
            text = (
                text[:pos]
                + rng.choice(["=", "==", "A", " ", "\n", "-", "_", "*", "\xe9", "===", "A="])
                + text[pos:]
            )
        elif roll < 0.55 and text:
            text = text[:-1]
        elif roll < 0.65:
            text = text.rstrip("=")
        b64.append(text)
    b64 += [
        "",
        "=",
        "==",
        "A",
        "AA",
        "AA=",
        "AA==",
        "AAA=",
        "AAAA",
        "AAAA=",
        "AAAA==",
        "AAAA=A",
        "A===",
        "AB==C",
        "/9j/2Q==",
    ]
    b64_cases = []
    for text in b64:
        try:
            decoded = base64.b64decode(text, validate=True)
            b64_cases.append([text, decoded.hex()])
        except (ValueError, Exception):  # noqa: BLE001
            b64_cases.append([text, None])

    checker = jsonschema.FormatChecker()
    uuids = [
        "11111111-2222-4333-8444-555555555555",
        "F47AC10B-58CC-4372-A567-0E02B2C3D479",
        "{11111111-2222-4333-8444-555555555555}",
        "urn:uuid:11111111-2222-4333-8444-555555555555",
        "1111_111-2222-4333-8444-555555555555",
        "11111111222243338444555555555555",
        " 1111111-2222-4333-8444-555555555555",
        "+1111111-2222-4333-8444-555555555555",
        "0x111111-2222-4333-8444-555555555555",
        "0x_11111-2222-4333-8444-555555555555",
        "11111111-2222-4333-8444-55555555555g",
        "11111111-2222-4333-8444-5555555555555",
        "11111111-2222-4333-8444-55555555555",
        "",
        "not-a-uuid",
        "11111111-2222-4333-8444-555555555555\n",
        "11111111-2222-4333-8444-55555555555 ",
        "uuid:1111-2222-4333-8444-555555555555",
        "11111111--222-4333-8444-555555555555",
        "1111111_-2222-4333-8444-555555555555",
    ]
    for _ in range(300):
        u = str(uuid.UUID(int=rng.getrandbits(128)))
        pos = rng.randrange(len(u))
        u = u[:pos] + rng.choice(["", "-", "_", " ", "x", "0", "{", "}"]) + u[pos + 1 :]
        uuids.append(u)
    uuid_cases = [[u, checker.conforms(u, "uuid")] for u in uuids]
    return {
        "repr_strings": repr_cases,
        "repr_values": value_cases,
        "round": floats,
        "b64": b64_cases,
        "uuid": uuid_cases,
    }


def gen_uart():
    """The JSON body and md5 `Machine.send_json_with_hash` writes for the
    profiles tests/test_profile_uart_payload.py and test_api_profiles.py load."""
    docs = [
        ("adaptive", adaptive_profile(296_904, 10_000)),
        ("simple", json.loads((REPO / "simple_profile.json").read_text())),
        ("example", json.loads((REPO / "profile_schema" / "example_profile.json").read_text())),
        ("manual_mode", ProfileManager.build_manual_mode_profile()),
    ]
    out = []
    for name, doc in docs:
        outcome = esp32_outcome(doc, DEFAULT_SETTINGS, 100)
        body = json.dumps(SENT[0])
        assert outcome["md5"] == md5(body)
        out.append(
            {
                "name": name,
                "body": body,
                "md5": md5(body),
                "profile_json_len": len(json.dumps(doc).encode("utf-8")),
            }
        )
    return out


def main():
    write("uart.json", gen_uart())
    write("validation.json", gen_validation())
    write("preprocessor.json", gen_preprocessor())
    write("esp32.json", gen_esp32())
    write("manual_shots.json", gen_manual_shots())
    write("profiles_helpers.json", gen_profiles_helpers())
    write("pour_over.json", gen_pour_over())
    write("primitives.json", gen_primitives())


if __name__ == "__main__":
    main()
