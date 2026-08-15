import pytest

from config import (
    TARE_BEHAVIOR_AFTER_RETRACTION,
    TARE_BEHAVIOR_BEFORE_RETRACTION,
)
from settings_validation import validate_partial_retraction, validate_tare_behavior


@pytest.mark.parametrize(
    "behavior",
    [TARE_BEHAVIOR_AFTER_RETRACTION, TARE_BEHAVIOR_BEFORE_RETRACTION],
)
def test_tare_behavior_accepts_supported_values(behavior):
    validate_tare_behavior(behavior)


def test_tare_behavior_rejects_unknown_value():
    with pytest.raises(ValueError, match="unsupported tare behavior"):
        validate_tare_behavior("during_retraction")


@pytest.mark.parametrize("distance", [36.26, 45.33, 67.99])
def test_partial_retraction_accepts_supported_range(distance):
    validate_partial_retraction(distance)


@pytest.mark.parametrize("distance", [36.25, 68.0, float("nan"), float("inf")])
def test_partial_retraction_rejects_values_outside_supported_range(distance):
    with pytest.raises(ValueError, match="partial_retraction must be between"):
        validate_partial_retraction(distance)
