import math

from config import (
    PROFILE_PARTIAL_RETRACTION_MAX,
    PROFILE_PARTIAL_RETRACTION_MIN,
    SHOT_DATA_SHARING,
    TARE_BEHAVIORS,
)

# Settings that start as None ("not answered yet") and only accept booleans
# once the user answers. The generic type check would otherwise reject the
# first answer because type(True) is not type(None).
TRI_STATE_BOOL_SETTINGS = frozenset({SHOT_DATA_SHARING})


def is_valid_setting_type(setting_name: str, value, current_value) -> bool:
    if setting_name in TRI_STATE_BOOL_SETTINGS:
        return isinstance(value, bool)
    return type(value) is type(current_value)


def validate_tare_behavior(value: str) -> None:
    if not isinstance(value, str) or value not in TARE_BEHAVIORS:
        raise ValueError(f"unsupported tare behavior: {value}")


def validate_partial_retraction(value: float) -> None:
    if (
        not math.isfinite(value)
        or not PROFILE_PARTIAL_RETRACTION_MIN <= value <= PROFILE_PARTIAL_RETRACTION_MAX
    ):
        raise ValueError(
            "partial_retraction must be between "
            f"{PROFILE_PARTIAL_RETRACTION_MIN:.2f} and "
            f"{PROFILE_PARTIAL_RETRACTION_MAX:.2f} mm"
        )
