"""Turn a recorded manual brew into a saveable espresso profile.

During a manual brew the encoder drives the pressure or flow target and the
backend records it like any other setpoint. Replaying those recorded targets as
the points of a curve reproduces what the user actually did, so no separate
recorder is needed: the shot history already holds everything.

The user can hop between the pressure and the flow stage as often as they like,
so the samples are cut into runs of consecutive samples under the same control
and each run becomes one stage. Replaying the whole shot as a single stage would
lose the switches; replaying each sample as its own stage would lose the curve.

Implements section 5 ("Profile construction") of the Manual mode cross-repo
contract.
"""

import math
import uuid
from datetime import datetime

from log import MeticulousLogger
from profiles import (
    MANUAL_MODE_AUTHOR_ID,
    MANUAL_MODE_FINAL_WEIGHT_SENTINEL,
    MANUAL_MODE_PROFILE_ID,
    MANUAL_MODE_TEMPERATURE,
)
from shot_database import SearchOrder, SearchParams, ShotDataBase

logger = MeticulousLogger.getLogger(__name__)

MAX_PROFILE_NAME_LENGTH = 64
MIN_STAGE_DURATION = 0.1
MIN_MEANINGFUL_WEIGHT = 1.0


class ManualShotNotFound(Exception):
    """No history entry was brewed with the Manual mode profile."""


class ManualShotHasNoTargets(Exception):
    """The manual brew recorded no pressure or flow targets to build a stage from."""


def _is_number(value) -> bool:
    """True for a finite int/float. Booleans are not pressures or weights."""
    return (
        isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)
    )


def find_manual_shot(shot_id: str | None = None) -> dict:
    """Latest history entry brewed with the Manual mode profile, or the given one."""
    params = SearchParams(
        ids=[shot_id] if shot_id else [MANUAL_MODE_PROFILE_ID],
        sort=SearchOrder.descending,
        max_results=1,
        dump_data=True,
    )
    results = ShotDataBase.search_history(params)
    if not results:
        raise ManualShotNotFound("no manual brew in history")

    shot = results[0]
    if shot_id and (shot.get("profile") or {}).get("id") != MANUAL_MODE_PROFILE_ID:
        raise ManualShotNotFound("the requested shot was not brewed in manual mode")

    return shot


def _target_runs(shot: dict) -> list:
    """The shot's target samples, cut into runs of consecutive samples per control.

    Each run is `{"type": "pressure" | "flow", "points": [[x, y], ...],
    "last_sample": <the shot dict of the newest sample kept>}`, with `x` in
    seconds from the run's own first sample.
    """
    runs: list[dict] = []

    for sample in shot.get("data") or []:
        if not isinstance(sample, dict):
            continue
        recorded = sample.get("shot")
        if not isinstance(recorded, dict):
            continue
        setpoints = recorded.get("setpoints")
        if not isinstance(setpoints, dict):
            continue
        active = setpoints.get("active")
        if active not in ("pressure", "flow"):
            continue
        target = setpoints.get(active)
        if not _is_number(target):
            continue
        timestamp = sample.get("profile_time", sample.get("time"))
        if not _is_number(timestamp):
            continue

        if not runs or runs[-1]["type"] != active:
            runs.append(
                {
                    "type": active,
                    "points": [],
                    "start_time": timestamp,
                    "last_x": None,
                    "last_sample": None,
                }
            )
        run = runs[-1]

        # The sample carries a usable target, so it is the newest one seen even
        # if its timestamp does not advance the curve.
        run["last_sample"] = recorded

        x = round((timestamp - run["start_time"]) / 1000.0, 2)
        if run["last_x"] is not None and x <= run["last_x"]:
            continue

        run["points"].append([x, round(target, 2)])
        run["last_x"] = x

    return runs


def _stage_from_run(run: dict, index: int) -> dict:
    """One stage replaying `run`, numbered `index` (1-based) among all runs."""
    duration = run["points"][-1][0]
    return {
        "name": f"{run['type'].capitalize()} {index}",
        "key": str(uuid.uuid4()),
        "type": run["type"],
        "dynamics": {
            "points": run["points"],
            "over": "time",
            "interpolation": "none",
        },
        "exit_triggers": [
            {
                "type": "time",
                "value": max(duration, MIN_STAGE_DURATION),
                "relative": True,
                "comparison": ">=",
            }
        ],
        "limits": [],
    }


def build_profile_from_manual_shot(shot: dict, name: str | None = None) -> dict:
    """Pure: a history entry (search_history shape, dump_data=True) -> profile dict."""
    runs = _target_runs(shot)
    if not runs:
        raise ManualShotHasNoTargets("the manual brew recorded no pressure or flow targets")

    stages = [_stage_from_run(run, index) for index, run in enumerate(runs, start=1)]

    weight = runs[-1]["last_sample"].get("weight")
    if _is_number(weight) and weight >= MIN_MEANINGFUL_WEIGHT:
        final_weight = round(weight, 1)
    else:
        final_weight = MANUAL_MODE_FINAL_WEIGHT_SENTINEL

    source_profile = shot.get("profile") or {}
    temperature = source_profile.get("temperature")
    if not _is_number(temperature):
        temperature = MANUAL_MODE_TEMPERATURE

    if isinstance(name, str) and name.strip():
        profile_name = name.strip()[:MAX_PROFILE_NAME_LENGTH]
    else:
        profile_name = datetime.fromtimestamp(shot["time"]).strftime("Manual %Y-%m-%d %H:%M")

    points_kept = sum(len(run["points"]) for run in runs)
    logger.info(
        f"Built a profile from a manual brew: {len(stages)} stages, {points_kept} targets"
    )

    return {
        "id": str(uuid.uuid4()),
        "name": profile_name,
        "author": source_profile.get("author") or "",
        "author_id": source_profile.get("author_id") or MANUAL_MODE_AUTHOR_ID,
        "previous_authors": [],
        "temperature": temperature,
        "final_weight": final_weight,
        "variables": [],
        "display": {},
        "stages": stages,
    }
