import math

from config import (
    PROFILE_PARTIAL_RETRACTION_MAX,
    PROFILE_PARTIAL_RETRACTION_MIN,
    TARE_BEHAVIORS,
)


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
