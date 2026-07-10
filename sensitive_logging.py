"""Helpers for logging connectivity failures without exposing credentials."""

import os
from collections.abc import Sequence


def credential_metadata(ssid: str, password: str) -> str:
    return (
        f"ssid_bytes={len(ssid.encode('utf-8'))}, "
        f"password_bytes={len(password.encode('utf-8'))}"
    )


def payload_metadata(payload) -> str:
    if isinstance(payload, (list, tuple)):
        size = sum(len(part) for part in payload)
    else:
        size = len(payload)
    return f"{size} bytes"


def exception_metadata(error: BaseException) -> str:
    return type(error).__name__


def command_metadata(command: Sequence[object]) -> str:
    if not command:
        return "<empty command>"
    executable = os.path.basename(str(command[0]))
    return f"{executable} ({max(0, len(command) - 1)} args)"
