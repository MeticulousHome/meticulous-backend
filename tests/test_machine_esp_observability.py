import pytest

from esp_observability import (
    ESPCommunicationPhase,
    ESPObservability,
    should_start_firmware_update,
)


def test_stale_esp_info_during_update_recovery_does_not_start_another_update():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.0.0")
    monitor.begin_update("2.0.0", "1.0.0", 1)
    monitor.finish_flashing(2)

    assert monitor.observe_valid_message("ESPInfo", 3, "1.0.0") == []
    assert monitor.phase == ESPCommunicationPhase.WAITING_FOR_BOOT
    assert monitor.expected_firmware == "2.0.0"
    assert not should_start_firmware_update(
        available_firmware="2.0.0",
        running_firmware="1.0.0",
        update_in_progress=monitor.update_in_progress,
        flashing_disallowed=False,
    )


@pytest.mark.parametrize(
    (
        "available_firmware",
        "running_firmware",
        "update_in_progress",
        "flashing_disallowed",
        "expected",
    ),
    [
        ("2.0.0", "1.0.0", False, False, True),
        ("2.0.0", "1.0.0", True, False, False),
        ("2.0.0", "2.0.0", False, False, False),
        ("2.0.0", "1.0.0", False, True, False),
        (None, "1.0.0", False, False, False),
    ],
)
def test_should_start_firmware_update(
    available_firmware,
    running_firmware,
    update_in_progress,
    flashing_disallowed,
    expected,
):
    assert (
        should_start_firmware_update(
            available_firmware,
            running_firmware,
            update_in_progress,
            flashing_disallowed,
        )
        is expected
    )
