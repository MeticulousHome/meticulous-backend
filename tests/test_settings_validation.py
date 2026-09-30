import pytest

from config import (
    TARE_BEHAVIOR_AFTER_RETRACTION,
    TARE_BEHAVIOR_BEFORE_RETRACTION,
)
from settings_validation import (
    is_valid_setting_type,
    validate_partial_retraction,
    validate_tare_behavior,
)


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


@pytest.mark.parametrize("current", [None, True, False])
@pytest.mark.parametrize("value", [True, False])
def test_shot_data_sharing_accepts_booleans_from_any_state(current, value):
    assert is_valid_setting_type("shot_data_sharing", value, current) is True


@pytest.mark.parametrize("value", [None, "true", 1, 0])
def test_shot_data_sharing_rejects_non_booleans(value):
    assert is_valid_setting_type("shot_data_sharing", value, None) is False


def test_other_settings_keep_strict_type_matching():
    assert is_valid_setting_type("ssh_enabled", True, False) is True
    assert is_valid_setting_type("ssh_enabled", 1, False) is False
    assert is_valid_setting_type("heating_timeout", 5, 10) is True
    assert is_valid_setting_type("heating_timeout", "5", 10) is False
