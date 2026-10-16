import re
import time
from collections import deque
from typing import Optional

from config import (
    CONFIG_SYSTEM,
    CONFIG_USER,
    MACHINE_SERIAL_NUMBER,
    UPDATE_CHANNEL,
    MeticulousConfig,
)
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

LIMITED_ACCESS_CHANNEL = "factory"
UNLOCKED_CHANNEL = "stable"
MASTER_UNLOCK_CODE = "met"
UNLOCK_CODE_XOR = 0xC0FFEE
UNLOCK_CODE_MODULO = 99999
UNLOCK_MAX_FAILURES = 10
UNLOCK_FAILURE_WINDOW_S = 60
UNLOCK_LOCKOUT_S = 30
_JS_INT_PREFIX = re.compile(r"^\s*([+-]?[0-9]+)")


def is_limited_access() -> bool:
    return MeticulousConfig[CONFIG_USER][UPDATE_CHANNEL] == LIMITED_ACCESS_CHANNEL


def _parse_int_prefix(value) -> Optional[int]:
    if value is None:
        return None
    match = _JS_INT_PREFIX.match(str(value))
    return int(match.group(1)) if match else None


def serial_unlock_code(serial) -> Optional[str]:
    serial_int = _parse_int_prefix(serial)
    if serial_int is None or serial_int < 0:
        return None
    return str((serial_int ^ UNLOCK_CODE_XOR) % UNLOCK_CODE_MODULO)


def expected_unlock_codes() -> set[str]:
    codes = {MASTER_UNLOCK_CODE}
    code = serial_unlock_code(MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER])
    if code is not None:
        codes.add(code)
    return codes


def validate_unlock_code(code) -> bool:
    return isinstance(code, str) and code.strip().lower() in expected_unlock_codes()


class UnlockThrottle:
    _failures: deque = deque()
    _locked_until: float = 0.0

    @classmethod
    def reset(cls) -> None:
        cls._failures.clear()
        cls._locked_until = 0.0

    @classmethod
    def retry_after(cls, now: Optional[float] = None) -> int:
        now = time.monotonic() if now is None else now
        remaining = cls._locked_until - now
        return int(remaining) + 1 if remaining > 0 else 0

    @classmethod
    def record_failure(cls, now: Optional[float] = None) -> None:
        now = time.monotonic() if now is None else now
        cls._failures.append(now)
        while cls._failures and now - cls._failures[0] > UNLOCK_FAILURE_WINDOW_S:
            cls._failures.popleft()
        if len(cls._failures) > UNLOCK_MAX_FAILURES:
            cls._locked_until = now + UNLOCK_LOCKOUT_S
            cls._failures.clear()
            logger.warning(
                "Unlock endpoint locked for %s s after repeated failures", UNLOCK_LOCKOUT_S
            )


def unlock() -> str:
    if not is_limited_access():
        return MeticulousConfig[CONFIG_USER][UPDATE_CHANNEL]
    from ota import UpdateManager
    from profiles import PROFILE_EVENT, ProfileManager

    UpdateManager.setChannel(UNLOCKED_CHANNEL)
    MeticulousConfig[CONFIG_USER][UPDATE_CHANNEL] = UNLOCKED_CHANNEL
    MeticulousConfig.save()
    ProfileManager._emit_profile_event(PROFILE_EVENT.RELOAD)
    logger.info("Machine unlocked: update channel set to %s", UNLOCKED_CHANNEL)
    return UNLOCKED_CHANNEL
