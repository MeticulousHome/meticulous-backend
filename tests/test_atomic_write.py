"""Protects settings and profiles on disk: a failed or interrupted write must leave the previous
file intact and readable, with its permissions, so a power cut never resets the machine."""

import os
import stat

import pytest
import yaml

import atomic_write
from atomic_write import atomic_write_text
from config import DefaultConfiguration_V1, MeticulousConfigDict


def test_failed_write_keeps_old_file_and_leaves_no_temp(tmp_path, monkeypatch):
    target = tmp_path / "config.yml"
    target.write_text("old")

    def power_cut(src, dst):
        raise OSError("power cut")

    monkeypatch.setattr(atomic_write.os, "replace", power_cut)
    with pytest.raises(OSError):
        atomic_write_text(target, "new")

    assert target.read_text() == "old"
    assert list(tmp_path.iterdir()) == [target]


def test_existing_mode_is_kept_and_new_file_is_world_readable(tmp_path):
    target = tmp_path / "a.txt"
    target.write_text("old")
    os.chmod(target, 0o640)
    atomic_write_text(target, "new")
    assert stat.S_IMODE(target.stat().st_mode) == 0o640

    fresh = tmp_path / "b.txt"
    atomic_write_text(fresh, "x")
    assert stat.S_IMODE(fresh.stat().st_mode) == 0o644


def test_config_save_round_trips_and_keeps_mode(tmp_path):
    path = tmp_path / "config.yml"
    path.write_text(yaml.safe_dump(DefaultConfiguration_V1))
    os.chmod(path, 0o644)
    config = MeticulousConfigDict(path, DefaultConfiguration_V1)
    config["user"]["marker"] = "kept"
    config.save()

    assert yaml.safe_load(path.read_text())["user"]["marker"] == "kept"
    assert stat.S_IMODE(path.stat().st_mode) == 0o644


def test_config_save_interrupted_mid_dump_keeps_previous_file(tmp_path, monkeypatch):
    path = tmp_path / "config.yml"
    path.write_text(yaml.safe_dump(DefaultConfiguration_V1))
    config = MeticulousConfigDict(path, DefaultConfiguration_V1)
    before = path.read_text()

    def broken_dump(*args, **kwargs):
        raise RuntimeError("dictionary changed size during iteration")

    monkeypatch.setattr(yaml, "dump", broken_dump)
    with pytest.raises(RuntimeError):
        config.save()

    assert path.read_text() == before
    assert yaml.safe_load(before)["version"] == DefaultConfiguration_V1["version"]
