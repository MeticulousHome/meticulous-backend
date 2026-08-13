import importlib
import sys
from unittest.mock import MagicMock

# GPIO access is available only on the target machine.
sys.modules.setdefault("gpiod", MagicMock())
api_machine = importlib.import_module("api.machine")


def test_factory_reset_cleanup_preserves_hidden_identity_cache(tmp_path):
    identity_cache = tmp_path / ".device-identity" / "device-uuid"
    identity_cache.parent.mkdir()
    identity_cache.write_text(
        "123e4567-e89b-42d3-a456-426614174000\n",
        encoding="ascii",
    )
    ordinary_file = tmp_path / "settings.json"
    ordinary_file.write_text("user data", encoding="utf-8")
    ordinary_directory = tmp_path / "profiles"
    ordinary_directory.mkdir()
    (ordinary_directory / "profile.json").write_text("{}", encoding="utf-8")

    api_machine.cleanup_factory_reset_data(tmp_path)

    assert identity_cache.read_text(encoding="ascii") == (
        "123e4567-e89b-42d3-a456-426614174000\n"
    )
    assert not ordinary_file.exists()
    assert not ordinary_directory.exists()
