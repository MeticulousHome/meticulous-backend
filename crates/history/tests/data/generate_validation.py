"""Pydantic's answers for SearchParams and PourOverSession, for crates/history.

Writes validation.json: each case is an input and either the validated
model (`model_dump(mode="json")`) or the list of errors pydantic reports
(`json.loads(error.json(include_url=False))`). Non-finite floats are written as
the strings "NaN", "Infinity" and "-Infinity" under a {"$float": ...} marker.

    uv run python crates/history/tests/data/generate_validation.py
"""

import copy
import json
import math

from pyenv import write_json
from pour_over_history import PourOverSession
from pydantic import ValidationError
from shot_database import SearchParams


def encode(value):
    if isinstance(value, float) and not math.isfinite(value):
        return {"$float": repr(value).replace("nan", "NaN").replace("inf", "Infinity")}
    if isinstance(value, dict):
        return {k: encode(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [encode(v) for v in value]
    return value


def run(model, data):
    try:
        if model is SearchParams:
            instance = SearchParams(**data)
            dumped = instance.model_dump(mode="python")
            dumped["order_by"] = [o.value for o in instance.order_by]
            dumped["sort"] = instance.sort.value
            return {"input": encode(data), "ok": encode(dumped)}
        instance = model.model_validate(data)
        return {"input": encode(data), "ok": encode(instance.model_dump(mode="json"))}
    except ValidationError as error:
        errors = json.loads(error.json(include_url=False))
        return {"input": encode(data), "errors": encode(errors)}


SEARCH_CASES = [
    {},
    {"sort": "sideways", "max_results": "x"},
    {"ids": "abc"},
    {"ids": [1, "2", 3.0, True]},
    {"ids": [1, None]},
    {"ids": [{"a": 1}]},
    {"start_date": "1760000000.5", "end_date": True},
    {"start_date": 1760000000, "end_date": 1760000100.25},
    {"start_date": None},
    {"start_date": "abc"},
    {"start_date": [1]},
    {"order_by": ["date", "profile"]},
    {"order_by": ["date", "profile", "x"]},
    {"order_by": "date"},
    {"order_by": []},
    {"dump_data": "false"},
    {"dump_data": "yes"},
    {"dump_data": "maybe"},
    {"dump_data": "0"},
    {"dump_data": "OFF"},
    {"dump_data": 0},
    {"dump_data": 1},
    {"dump_data": 2},
    {"dump_data": 1.0},
    {"dump_data": None},
    {"max_results": "20"},
    {"max_results": " 7 "},
    {"max_results": "+3"},
    {"max_results": "1_000"},
    {"max_results": "-5"},
    {"max_results": 1.5},
    {"max_results": 2.0},
    {"max_results": True},
    {"max_results": None},
    {"max_results": "1e3"},
    {"max_results": "2.0"},
    {"query": 5},
    {"query": "bloom"},
    {"query": None},
    {"query": ""},
    {"extra": 1},
    {"sort": "asc"},
    {"sort": "ASC"},
    {"sort": 1},
    {"query": None, "ids": None},
    {"query": 1, "ids": 2, "sort": 3, "max_results": "z", "dump_data": "q", "order_by": 4},
]


def session():
    return {
        "schemaVersion": 4,
        "id": "11111111-2222-4333-8444-555555555555",
        "brewType": "pour_over",
        "mode": "free_pour",
        "name": "Free Pour",
        "source": "dial",
        "startedAt": "2026-08-16T08:00:00.000Z",
        "completedAt": "2026-08-16T08:02:15.000Z",
        "recipe": {
            "profileId": None,
            "profileName": "Free Pour",
            "doseG": 15,
            "temperatureC": 92,
            "targetWaterG": None,
            "targetDurationMs": None,
            "pourTargets": [],
        },
        "measurements": {
            "serverBaselineG": -0.4,
            "brewerG": 134.2,
            "setupG": 149.2,
            "doseG": 15.1,
            "waterTemperatureC": 92,
            "waterPouredG": 225.3,
            "beverageG": 196.8,
            "retainedG": 28.5,
            "durationMs": 135_000,
            "status": "measured",
        },
        "pours": [
            {
                "number": 1,
                "startTimeMs": 0,
                "endTimeMs": 10_000,
                "startWeightG": 0,
                "endWeightG": 40.2,
                "waterG": 40.2,
                "averageFlowGps": 4.02,
                "peakFlowGps": 4.8,
            }
        ],
        "samples": [
            {"t": 0, "w": 0, "f": 0.8, "p": 1},
            {"t": 10_000, "w": 40.2, "f": 0.1, "p": 0},
        ],
        "completion": "brewer_removed",
        "sync": {"status": "pending", "attempts": 0},
    }


def variant(path, value=None, delete=False):
    data = session()
    target = data
    for key in path[:-1]:
        target = target[key]
    if delete:
        del target[path[-1]]
    else:
        target[path[-1]] = value
    return data


def pour_over_cases():
    cases = [session()]
    profile = session()
    profile["mode"] = "profile"
    profile["name"] = "Three Pour 1:15"
    profile["recipe"]["profileId"] = "profile-1"
    profile["recipe"]["targetWaterG"] = 225
    profile["recipe"]["targetDurationMs"] = 135_000
    profile["recipe"]["pourTargets"] = [
        {"number": 1, "startTimeMs": 0, "stopWeightG": 40, "flowGps": 4},
        {"number": 2, "startTimeMs": 30000, "stopWeightG": 120, "flowRangeGps": [3, 4.5]},
    ]
    cases.append(profile)
    legacy = session()
    legacy["schemaVersion"] = 2
    legacy["measurements"]["emptyServerG"] = 182.4
    legacy["measurements"].pop("serverBaselineG")
    legacy["recipe"].pop("temperatureC")
    legacy["recipe"].pop("targetDurationMs")
    legacy["measurements"].pop("doseG")
    legacy["measurements"].pop("waterTemperatureC")
    cases.append(legacy)
    extra = session()
    extra["appVersion"] = "1.2.3"
    extra["samples"][0]["extraSample"] = {"nested": [1, 2]}
    extra["recipe"]["notes"] = "café"
    extra["sync"]["device"] = None
    cases.append(extra)
    uploaded = session()
    uploaded["sync"] = {
        "status": "uploaded",
        "attempts": 3,
        "uploadedAt": "2026-08-16T10:05:00+02:00",
    }
    cases.append(uploaded)
    cases += [
        variant(["samples"], []),
        variant(["samples"], session()["samples"] * 1_501),
        variant(["samples"], "not a list"),
        variant(["samples", 0, "f"], -1),
        variant(["samples", 0, "f"], float("inf")),
        variant(["samples", 0, "f"], float("nan")),
        variant(["samples", 0, "f"], "0.5"),
        variant(["samples", 0, "f"], " 0.5 "),
        variant(["samples", 0, "f"], "abc"),
        variant(["samples", 0, "f"], None),
        variant(["samples", 0, "f"], True),
        variant(["samples", 0, "p"], 1.0),
        variant(["samples", 0, "p"], 1.5),
        variant(["samples", 0, "p"], "3"),
        variant(["samples", 0, "p"], True),
        variant(["samples", 0, "p"], -1),
        variant(["samples", 0, "t"], 600_001),
        variant(["samples", 0], "sample"),
        variant(["measurements", "durationMs"], 600_001),
        variant(["measurements", "status"], "other"),
        variant(["measurements", "status"], delete=True),
        variant(["measurements", "serverBaselineG"], delete=True),
        variant(["measurements", "beverageG"], delete=True),
        variant(["measurements", "doseG"], 0),
        variant(["completedAt"], "2026-08-16T07:59:59.000Z"),
        variant(["name"], delete=True),
        variant(["name"], ""),
        variant(["name"], "x" * 513),
        variant(["name"], 5),
        variant(["id"], 123),
        variant(["id"], "é" * 256),
        variant(["brewType"], "espresso"),
        variant(["schemaVersion"], 5),
        variant(["schemaVersion"], "4"),
        variant(["schemaVersion"], 4.0),
        variant(["mode"], 1),
        variant(["source"], None),
        variant(["completion"], "unknown"),
        variant(["startedAt"], "2026-08-16T08:00:00"),
        variant(["startedAt"], "2026-08-16 08:00:00Z"),
        variant(["startedAt"], "2026-08-16T08:00:00.123Z"),
        variant(["startedAt"], "2026-08-16T08:00:00.1234567Z"),
        variant(["startedAt"], "2026-08-16T08:00:00+00:00"),
        variant(["startedAt"], "2026-08-16T09:30:00+01:30"),
        variant(["startedAt"], "2026-08-16T08:00:00z"),
        variant(["startedAt"], "not a date"),
        variant(["startedAt"], "2026-02-30T08:00:00Z"),
        variant(["startedAt"], 1786867200),
        variant(["startedAt"], None),
        variant(["recipe"], "recipe"),
        variant(["recipe"], None),
        variant(
            ["recipe", "pourTargets"],
            [{"number": 1, "startTimeMs": 0, "stopWeightG": 40, "flowRangeGps": [5, 4]}],
        ),
        variant(
            ["recipe", "pourTargets"],
            [{"number": 1, "startTimeMs": 0, "stopWeightG": 40, "flowRangeGps": [1]}],
        ),
        variant(
            ["recipe", "pourTargets"],
            [{"number": 1, "startTimeMs": 0, "stopWeightG": 40, "flowRangeGps": [1, 2, 3]}],
        ),
        variant(
            ["recipe", "pourTargets"],
            [{"number": 1, "startTimeMs": 0, "stopWeightG": 40, "flowRangeGps": "x"}],
        ),
        variant(
            ["recipe", "pourTargets"], [{"number": 0, "startTimeMs": 0, "stopWeightG": 40}]
        ),
        variant(
            ["recipe", "pourTargets"],
            [{"number": 1, "startTimeMs": 0, "stopWeightG": 40}] * 129,
        ),
        variant(["recipe", "targetWaterG"], delete=True),
        variant(["recipe", "doseG"], 0),
        variant(["recipe", "profileName"], ""),
        variant(["pours", 0, "endTimeMs"], 5),
        variant(["pours", 0, "number"], 0),
        variant(["sync", "status"], "done"),
        variant(["sync", "attempts"], -1),
        variant(["sync", "uploadedAt"], "2026-08-16T08:05:00"),
        variant(["sync"], None),
        [session()],
        "session",
        None,
        {},
    ]
    many = session()
    many["samples"][0]["f"] = -1
    many["samples"][1]["p"] = "x"
    many["name"] = ""
    many["sync"]["attempts"] = None
    cases.append(many)
    return cases


def main():
    reference = {
        "search_params": [run(SearchParams, copy.deepcopy(case)) for case in SEARCH_CASES],
        "pour_over": [run(PourOverSession, case) for case in pour_over_cases()],
    }
    write_json("validation.json", reference)


if __name__ == "__main__":
    main()
