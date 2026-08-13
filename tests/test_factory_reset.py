import importlib
from pathlib import Path
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


def test_factory_reset_cleanup_continues_after_deletion_error(tmp_path, monkeypatch, caplog):
    undeletable_file = tmp_path / "undeletable.json"
    undeletable_file.write_text("in use", encoding="utf-8")
    removable_file = tmp_path / "removable.json"
    removable_file.write_text("user data", encoding="utf-8")
    original_iterdir = Path.iterdir
    original_unlink = Path.unlink

    def ordered_iterdir(path):
        if path == tmp_path:
            return iter((undeletable_file, removable_file))
        return original_iterdir(path)

    def unlink_with_error(path, *args, **kwargs):
        if path == undeletable_file:
            raise OSError("file is busy")
        return original_unlink(path, *args, **kwargs)

    monkeypatch.setattr(Path, "iterdir", ordered_iterdir)
    monkeypatch.setattr(Path, "unlink", unlink_with_error)

    api_machine.cleanup_factory_reset_data(tmp_path)

    assert undeletable_file.exists()
    assert not removable_file.exists()
    assert "Could not remove factory reset entry" in caplog.text
