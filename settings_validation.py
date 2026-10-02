import math

from config import (
    PROFILE_PARTIAL_RETRACTION_MAX,
    PROFILE_PARTIAL_RETRACTION_MIN,
    REPORT_CONTACT_MAIL,
    SHOT_DATA_SHARING,
    TARE_BEHAVIORS,
)

# Settings that start as None ("not answered yet") and only accept booleans
# once the user answers. The generic type check would otherwise reject the
# first answer because type(True) is not type(None).
TRI_STATE_BOOL_SETTINGS = frozenset({SHOT_DATA_SHARING})

# Settings that hold a string the user may clear again, so both str and None
# are valid regardless of the current value.
NULLABLE_STR_SETTINGS = frozenset({REPORT_CONTACT_MAIL})

# RFC 5321 caps the whole address at 254 octets in practice.
REPORT_CONTACT_MAIL_MAX_LENGTH = 254


def is_valid_setting_type(setting_name: str, value, current_value) -> bool:
    if setting_name in TRI_STATE_BOOL_SETTINGS:
        return isinstance(value, bool)
    if setting_name in NULLABLE_STR_SETTINGS:
        return value is None or isinstance(value, str)
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


def normalize_report_contact_mail(value) -> str | None:
    """Trim the address and map an empty one to None.

    Only the shape is checked (one `@` with something on both sides, no
    whitespace): the address is for support to reply to, not for delivery
    from the machine, so a stricter check would just reject valid mailboxes.
    """
    if value is None:
        return None
    if not isinstance(value, str):
        raise ValueError("report_contact_mail must be a string or null")
    address = value.strip()
    if not address:
        return None
    if len(address) > REPORT_CONTACT_MAIL_MAX_LENGTH:
        raise ValueError(
            f"report_contact_mail must be at most {REPORT_CONTACT_MAIL_MAX_LENGTH} characters"
        )
    local_part, separator, domain = address.partition("@")
    if (
        not separator
        or not local_part
        or not domain
        or "@" in domain
        or any(char.isspace() for char in address)
    ):
        raise ValueError("report_contact_mail must look like name@domain")
    return address
