from copy import deepcopy
from typing import Any


STRICT_GREATER_THAN_TRIGGER_TYPES = frozenset(
    {"weight", "time", "pressure", "flow", "piston_position", "power"}
)


def canonicalize_exit_trigger_comparisons(profile: Any) -> tuple[Any, bool]:
    """Return a canonicalized copy and whether an affected comparison changed."""

    migrated = deepcopy(profile)
    changed = False
    if not isinstance(migrated, dict):
        return migrated, changed

    stages = migrated.get("stages")
    if not isinstance(stages, list):
        return migrated, changed

    for stage in stages:
        if not isinstance(stage, dict):
            continue
        exit_triggers = stage.get("exit_triggers")
        if not isinstance(exit_triggers, list):
            continue
        for trigger in exit_triggers:
            if not isinstance(trigger, dict):
                continue
            if (
                trigger.get("type") in STRICT_GREATER_THAN_TRIGGER_TYPES
                and ("comparison" not in trigger or trigger["comparison"] == ">=")
            ):
                trigger["comparison"] = ">"
                changed = True

    return migrated, changed


def migrate_legacy_exit_trigger_comparisons(profile: Any) -> Any:
    """Return a copy with affected legacy or omitted comparisons made strict.

    Complex-profile operators, limits, unrelated trigger types, and malformed containers
    are intentionally left untouched. This makes the migration safe to apply repeatedly
    at every simplified-profile persistence boundary.
    """

    migrated, _ = canonicalize_exit_trigger_comparisons(profile)
    return migrated
