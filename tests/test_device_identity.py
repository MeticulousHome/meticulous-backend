import os
import stat

import pytest

import device_identity


FIRST_UUID = "123e4567-e89b-42d3-a456-426614174000"
SECOND_UUID = "987e6543-e21b-45d3-b654-426614174999"


@pytest.fixture
def cache_path(tmp_path, monkeypatch):
    path = tmp_path / ".device-identity" / "device-uuid"
    monkeypatch.setattr(device_identity, "DEVICE_UUID_CACHE_PATH", path)
    return path


def test_validates_canonical_uuid_v4():
    assert device_identity.is_valid_device_uuid(FIRST_UUID)
    assert not device_identity.is_valid_device_uuid("")
    assert not device_identity.is_valid_device_uuid(FIRST_UUID.upper())
    assert not device_identity.is_valid_device_uuid("123e4567-e89b-32d3-a456-426614174000")


def test_creates_and_reuses_cache(cache_path):
    assert device_identity.update_device_uuid_cache(FIRST_UUID)
    assert cache_path.read_text(encoding="ascii") == f"{FIRST_UUID}\n"
    if os.name != "nt":
        assert stat.S_IMODE(cache_path.parent.stat().st_mode) == 0o700
        assert stat.S_IMODE(cache_path.stat().st_mode) == 0o600

    cache_path.parent.chmod(0o755)
    cache_path.chmod(0o644)
    assert not device_identity.update_device_uuid_cache(FIRST_UUID)
    if os.name != "nt":
        assert stat.S_IMODE(cache_path.parent.stat().st_mode) == 0o700
        assert stat.S_IMODE(cache_path.stat().st_mode) == 0o600


def test_esp_uuid_overwrites_different_var_som_cache(cache_path):
    cache_path.parent.mkdir()
    cache_path.write_text(f"{FIRST_UUID}\n", encoding="ascii")

    assert device_identity.update_device_uuid_cache(SECOND_UUID)
    assert cache_path.read_text(encoding="ascii") == f"{SECOND_UUID}\n"


def test_rejects_invalid_uuid_without_changing_cache(cache_path):
    cache_path.parent.mkdir()
    cache_path.write_text(f"{FIRST_UUID}\n", encoding="ascii")

    with pytest.raises(ValueError):
        device_identity.update_device_uuid_cache("invalid")

    assert cache_path.read_text(encoding="ascii") == f"{FIRST_UUID}\n"
