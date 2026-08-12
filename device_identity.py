import os
import re
import tempfile
import uuid
from pathlib import Path


DEVICE_UUID_CACHE_PATH = Path(
    os.getenv("DEVICE_UUID_CACHE_PATH", "/meticulous-user/.device-identity/device-uuid")
)

DEVICE_UUID_PATTERN = re.compile(
    r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$"
)


def is_valid_device_uuid(device_uuid: str) -> bool:
    return bool(DEVICE_UUID_PATTERN.fullmatch(device_uuid))


def generate_device_uuid() -> str:
    """Generate a canonical UUIDv4 using the VAR-SOM OS entropy source."""
    return str(uuid.uuid4())


def get_device_uuid_assignment(
    reported_uuid: str,
    protocol_supported: bool,
    pending_assignment: str | None,
) -> str | None:
    """Return the UUID to assign without consulting the VAR-SOM cache."""
    if is_valid_device_uuid(reported_uuid) or not protocol_supported:
        return None
    if pending_assignment and is_valid_device_uuid(pending_assignment):
        return pending_assignment
    return generate_device_uuid()


def update_device_uuid_cache(device_uuid: str) -> bool:
    """Persist the ESP-owned UUID and return whether the cache changed."""
    if not is_valid_device_uuid(device_uuid):
        raise ValueError("Invalid ESP device UUID")

    try:
        cached_uuid = DEVICE_UUID_CACHE_PATH.read_text(encoding="ascii").strip()
    except (FileNotFoundError, UnicodeError):
        cached_uuid = ""

    if cached_uuid == device_uuid:
        DEVICE_UUID_CACHE_PATH.parent.chmod(0o700)
        DEVICE_UUID_CACHE_PATH.chmod(0o600)
        return False

    cache_directory = DEVICE_UUID_CACHE_PATH.parent
    cache_directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    cache_directory.chmod(0o700)

    file_descriptor, temporary_path_string = tempfile.mkstemp(
        prefix=".device-uuid.",
        dir=cache_directory,
        text=True,
    )
    temporary_path = Path(temporary_path_string)
    try:
        with os.fdopen(file_descriptor, "w", encoding="ascii") as temporary_file:
            temporary_file.write(f"{device_uuid}\n")
            temporary_file.flush()
            os.fsync(temporary_file.fileno())
        temporary_path.chmod(0o600)
        os.replace(temporary_path, DEVICE_UUID_CACHE_PATH)
    finally:
        temporary_path.unlink(missing_ok=True)

    return True
