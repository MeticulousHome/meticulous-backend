import smoke_validation as sv
from smoke_validation import SmokeValidationManager

FINAL_COFFEE_LOG = "coffee profile finished. Heater will be kept on until timeout."


def configure_state(monkeypatch, tmp_path):
    state_file = tmp_path / "smoke-validation.json"
    version_file = tmp_path / "image-build-version"
    version_file.write_text("2026M1234-nightly\n", encoding="utf-8")
    monkeypatch.setattr(sv, "STATE_FILE", state_file)
    monkeypatch.setattr(sv, "VERSION_FILE", version_file)
    monkeypatch.setattr(sv, "PUBLISH_COMMAND", str(tmp_path / "missing-publisher"))
    monkeypatch.setattr(sv.shutil, "which", lambda _: None)
    SmokeValidationManager._shot_sequence = {
        "active": False,
        "saw_heating": False,
        "saw_extraction": False,
        "saw_retracting": False,
        "saw_purge": False,
    }
    return version_file


def test_post_update_shot_state_resets_when_image_version_changes(monkeypatch, tmp_path):
    version_file = configure_state(monkeypatch, tmp_path)

    assert SmokeValidationManager.mark_post_update_shot_completed() is True
    state = SmokeValidationManager.get_state()
    assert state["post_update_shot_completed"] is True
    assert state["post_update_shot_completed_version"] == "2026M1234-nightly"

    version_file.write_text("2026M1235-nightly\n", encoding="utf-8")
    state = SmokeValidationManager.get_state()
    assert state["image_version"] == "2026M1235-nightly"
    assert state["post_update_shot_completed"] is False
    assert state["post_update_shot_completed_at"] is None
    assert state["post_update_shot_completed_version"] is None


def test_final_coffee_log_marks_completed_after_full_sequence(monkeypatch, tmp_path):
    configure_state(monkeypatch, tmp_path)

    SmokeValidationManager.observe_status("heating", extracting=False)
    SmokeValidationManager.observe_status("Preinfusion", extracting=True)
    SmokeValidationManager.observe_status("retracting", extracting=True)
    SmokeValidationManager.observe_status("purge", extracting=False)
    SmokeValidationManager.observe_esp_log(FINAL_COFFEE_LOG)

    assert SmokeValidationManager.get_state()["post_update_shot_completed"] is True


def test_final_coffee_log_does_not_mark_completed_without_purge(monkeypatch, tmp_path):
    configure_state(monkeypatch, tmp_path)

    SmokeValidationManager.observe_status("heating", extracting=False)
    SmokeValidationManager.observe_status("Preinfusion", extracting=True)
    SmokeValidationManager.observe_status("retracting", extracting=True)
    SmokeValidationManager.observe_esp_log(FINAL_COFFEE_LOG)

    assert SmokeValidationManager.get_state()["post_update_shot_completed"] is False


def test_final_coffee_log_requires_ordered_retracting_and_purge(monkeypatch, tmp_path):
    configure_state(monkeypatch, tmp_path)

    SmokeValidationManager.observe_status("heating", extracting=False)
    SmokeValidationManager.observe_status("purge", extracting=False)
    SmokeValidationManager.observe_status("retracting", extracting=True)
    SmokeValidationManager.observe_status("Preinfusion", extracting=True)
    SmokeValidationManager.observe_esp_log(FINAL_COFFEE_LOG)

    assert SmokeValidationManager.get_state()["post_update_shot_completed"] is False
