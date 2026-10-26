import pytest

from config import (
    TARE_BEHAVIOR_AFTER_RETRACTION,
    TARE_BEHAVIOR_BEFORE_RETRACTION,
)
from settings_validation import (
    is_valid_setting_type,
    normalize_report_contact_mail,
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


@pytest.mark.parametrize("current", [None, "a@b.c"])
@pytest.mark.parametrize("value", [None, "x@y.z", ""])
def test_report_contact_mail_accepts_null_or_string_from_any_state(current, value):
    assert is_valid_setting_type("report_contact_mail", value, current) is True


@pytest.mark.parametrize("value", [1, True, ["a"]])
@pytest.mark.parametrize("current", [None, "a@b.c"])
def test_report_contact_mail_rejects_other_types(current, value):
    assert is_valid_setting_type("report_contact_mail", value, current) is False


@pytest.mark.parametrize("value", [None, "", "   ", "\t\n"])
def test_report_contact_mail_normalizes_missing_values_to_none(value):
    assert normalize_report_contact_mail(value) is None


def test_report_contact_mail_is_trimmed():
    assert normalize_report_contact_mail(" user@example.com ") == "user@example.com"
    assert normalize_report_contact_mail("user@example.com") == "user@example.com"


def test_report_contact_mail_accepts_longest_allowed_address():
    address = "a" * 249 + "@b.cd"
    assert len(address) == 254
    assert normalize_report_contact_mail(address) == address


@pytest.mark.parametrize(
    "value",
    [
        "userexample.com",
        "@example.com",
        "user@",
        "@",
        "user@@example.com",
        "user@exa@mple.com",
        "us er@example.com",
        "user@exam ple.com",
        "a" * 250 + "@b.cd",
    ],
)
def test_report_contact_mail_rejects_malformed_addresses(value):
    with pytest.raises(ValueError):
        normalize_report_contact_mail(value)


@pytest.mark.parametrize("value", [123, True, ["user@example.com"]])
def test_report_contact_mail_rejects_non_strings(value):
    with pytest.raises(ValueError, match="string or null"):
        normalize_report_contact_mail(value)
