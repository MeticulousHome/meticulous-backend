"""Turn a recorded manual brew into a saveable espresso profile.

During a manual brew the encoder drives the pressure target and the backend
records it like any other setpoint. Replaying those recorded targets as the
points of a single pressure stage reproduces what the user actually did, so no
separate recorder is needed: the shot history already holds everything.

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
    """The manual brew recorded no pressure targets to build a stage from."""


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


def build_profile_from_manual_shot(shot: dict, name: str | None = None) -> dict:
    """Pure: a history entry (search_history shape, dump_data=True) -> profile dict."""
    points: list[list[float]] = []
    start_time = None
    last_x = None
    last_kept = None

    for sample in shot.get("data") or []:
        if not isinstance(sample, dict):
            continue
        recorded = sample.get("shot")
        if not isinstance(recorded, dict):
            continue
        setpoints = recorded.get("setpoints")
        if not isinstance(setpoints, dict):
            continue
        if setpoints.get("active") != "pressure":
            continue
        pressure = setpoints.get("pressure")
        if not _is_number(pressure):
            continue
        timestamp = sample.get("profile_time", sample.get("time"))
        if not _is_number(timestamp):
            continue

        # The sample carries a usable target, so it is the newest one seen even
        # if its timestamp does not advance the curve.
        last_kept = recorded

        if start_time is None:
            start_time = timestamp

        x = round((timestamp - start_time) / 1000.0, 2)
        if last_x is not None and x <= last_x:
            continue

        points.append([x, round(pressure, 2)])
        last_x = x

    if not points:
        raise ManualShotHasNoTargets("the manual brew recorded no pressure targets")

    duration = points[-1][0]
    exit_trigger = {
        "type": "time",
        "value": max(duration, MIN_STAGE_DURATION),
        "relative": True,
        "comparison": ">=",
    }

    weight = last_kept.get("weight")
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

    logger.info(f"Built a profile from a manual brew with {len(points)} pressure targets")

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
        "stages": [
            {
                "name": "Pressure",
                "key": str(uuid.uuid4()),
                "type": "pressure",
                "dynamics": {
                    "points": points,
                    "over": "time",
                    "interpolation": "none",
                },
                "exit_triggers": [exit_trigger],
                "limits": [],
            }
        ],
    }
