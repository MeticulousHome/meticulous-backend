import pytest

from esp_observability import ESPCommunicationPhase, ESPObservability

BOOT = "rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)"


def titles(events):
    return [event.title for event in events]


def capture_guru(monitor, reason="LoadProhibited", now=1):
    monitor.observe_raw_line(f"Guru Meditation Error: Core 1 panic'ed ({reason}).", now)
    monitor.observe_raw_line("Core 1 register dump:", now + 0.1)
    monitor.observe_raw_line("Backtrace: 0x40381234:0x3fceabcd", now + 0.2)


def test_stale_pre_update_info_does_not_finish_update():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", now=0.1, firmware_version="old")
    monitor.begin_update("new", "old", now=1)
    monitor.finish_flashing(now=10)

    assert monitor.observe_valid_message("ESPInfo", 10.1, "old") == []
    assert monitor.phase == ESPCommunicationPhase.WAITING_FOR_BOOT
    assert monitor.check_timeouts(12.5) == []


def test_update_requires_boot_and_exact_expected_esp_info_without_false_timeout():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "old")
    monitor.begin_update("new", "old", 1)
    monitor.finish_flashing(10)

    assert monitor.observe_raw_line(BOOT, 10.2) == []
    assert monitor.phase == ESPCommunicationPhase.WAITING_FOR_PROTOCOL
    assert monitor.check_timeouts(15) == []
    assert monitor.observe_boot_reason("SW", "3", 12) == []
    assert monitor.observe_valid_message("Data", 12.1) == []
    assert monitor.observe_valid_message("ESPInfo", 12.15, "new-dirty") == []
    assert monitor.phase == ESPCommunicationPhase.WAITING_FOR_PROTOCOL
    assert monitor.observe_valid_message("ESPInfo", 12.2, "new") == []
    assert monitor.phase == ESPCommunicationPhase.NORMAL


def test_update_without_expected_artifact_version_stays_in_recovery():
    monitor = ESPObservability(now=0)
    monitor.begin_update(None, "old", 1)
    monitor.finish_flashing(2)

    assert monitor.observe_raw_line(BOOT, 2.1) == []
    assert monitor.observe_valid_message("ESPInfo", 2.2, "new") == []
    assert monitor.phase == ESPCommunicationPhase.WAITING_FOR_PROTOCOL
    assert titles(monitor.check_timeouts(32.1)) == [
        "ESP32 did not recover after firmware update"
    ]


def test_update_recovery_failure_is_specific_and_bounded():
    monitor = ESPObservability(now=0)
    monitor.begin_update("new", "old", 1)
    monitor.finish_flashing(2)

    events = monitor.check_timeouts(32.1)

    assert titles(events) == ["ESP32 did not recover after firmware update"]
    assert events[0].tags["recovery_phase"] == "waiting_for_boot"
    assert events[0].context["timeout_seconds"] == 30.0


def test_update_without_current_info_retains_last_known_firmware():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")

    monitor.begin_update("2.0.0", None, 1)
    monitor.finish_flashing(2)
    event = monitor.check_timeouts(32.1)[0]

    assert monitor.previous_firmware == "1.2.3"
    assert event.context["previous_firmware"] == "1.2.3"


def test_flash_error_is_specific_bounded_and_restores_normal_state():
    monitor = ESPObservability(now=0)
    monitor.begin_update("new", "old", 1)

    event = monitor.fail_flashing("x" * 2000, "flash", 2)

    assert event.title == "ESP32 firmware update failed"
    assert len(event.context["detail"]) == 1000
    assert monitor.phase == ESPCommunicationPhase.NORMAL


def test_expected_api_reset_emits_no_error():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "current")
    monitor.begin_expected_reset(1)

    assert monitor.observe_raw_line(BOOT, 1.2) == []
    assert monitor.observe_boot_reason("SW", "3", 3) == []
    assert monitor.observe_valid_message("ESPInfo", 3.1, "current") == []
    assert monitor.phase == ESPCommunicationPhase.NORMAL


@pytest.mark.parametrize("message_type", ["Data", "Sensors"])
def test_any_valid_message_completes_non_update_expected_reset(message_type):
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "current")
    monitor.begin_expected_reset(1)

    assert monitor.observe_valid_message(message_type, 2) == []
    assert monitor.phase == ESPCommunicationPhase.NORMAL
    assert monitor.recovery_deadline is None
    assert monitor.check_timeouts(3.9) == []
    assert titles(monitor.check_timeouts(4.1)) == ["ESP32 valid-message timeout"]


def test_exact_debug_exception_is_structured_and_correlated_with_boot_reason():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")
    monitor.observe_raw_line(
        "Guru Meditation Error: Core 1 panic'ed (Unhandled debug exception).", 1
    )
    monitor.observe_raw_line(
        "Debug exception reason: Stack canary watchpoint triggered (Read ADC)", 1.1
    )
    monitor.observe_raw_line("Backtrace: 0x40381234:0x3fceabcd", 1.2)

    assert monitor.observe_raw_line(BOOT, 1.3) == []
    events = monitor.observe_boot_reason("PANIC", "4", 3.5)

    assert titles(events) == ["ESP32 firmware panic detected"]
    event = events[0]
    assert event.tags["reset_reason"] == "PANIC"
    assert event.tags["reset_reason_code"] == "4"
    assert event.context["core"] == "1"
    assert event.context["panic_reason"] == "Unhandled debug exception"
    assert event.context["exception_reason"] == "Stack canary watchpoint triggered"
    assert event.context["panic_location"] == "Read ADC"
    assert event.context["previous_firmware"] == "1.2.3"
    assert event.context["backtrace"].startswith("Backtrace:")
    assert event.fingerprint == "esp32-firmware-panic-stack-canary"


def test_abort_waits_for_boot_reason_and_emits_once():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")
    monitor.observe_raw_line("abort() was called at PC 0x420037e1 on core 1", 1)
    monitor.observe_raw_line("Backtrace: 0x4037801a:0x3fcebce0", 1.1)

    assert monitor.observe_raw_line(BOOT, 1.2) == []
    events = monitor.observe_boot_reason("PANIC", "4", 3)

    assert titles(events) == ["ESP32 firmware panic detected"]
    assert events[0].context["panic_reason"] == "abort"
    assert events[0].context["core"] == "1"
    assert events[0].context["panic_output"].startswith("abort() was called")
    assert events[0].fingerprint == "esp32-firmware-panic-abort"
    assert monitor.observe_valid_message("ESPBoot", 3.1) == []


@pytest.mark.parametrize(
    "panic_line",
    [
        "Guru Meditation Error: Core 1 panic'ed (LoadProhibited).",
        "abort() was called at PC 0x420037e1 on core 0",
    ],
    ids=["guru", "abort"],
)
def test_silent_panic_is_finalized_without_duplicate_timeout(panic_line):
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")

    monitor.observe_raw_line(panic_line, 1)

    assert monitor.phase == ESPCommunicationPhase.NORMAL
    events = monitor.check_timeouts(3.4)
    events += monitor.check_timeouts(4)

    assert titles(events) == ["ESP32 firmware panic detected"]
    assert monitor.observe_boot_reason("PANIC", "4", 5) == []


def test_delayed_boot_reason_after_silent_panic_remains_deduplicated():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")
    capture_guru(monitor, now=1)
    events = monitor.check_timeouts(3.3)

    events += monitor.observe_raw_line(BOOT, 4)
    events += monitor.observe_boot_reason("PANIC", "4", 4.1)
    events += monitor.observe_valid_message("ESPBoot", 4.1)

    assert titles(events) == ["ESP32 firmware panic detected"]
    assert monitor.phase == ESPCommunicationPhase.NORMAL


def test_normal_timeout_resumes_after_panic_recovery_traffic():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")
    capture_guru(monitor, now=1)
    assert titles(monitor.check_timeouts(3.3)) == ["ESP32 firmware panic detected"]

    assert monitor.observe_valid_message("Data", 4) == []
    assert titles(monitor.check_timeouts(6.1)) == ["ESP32 valid-message timeout"]


@pytest.mark.parametrize("line", ["Register dump:", "Backtrace: 0x1:0x2"])
def test_standalone_panic_evidence_does_not_suppress_timeout(line):
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")

    assert monitor.observe_raw_line(line, 1) == []
    events = monitor.check_timeouts(2.2)

    assert titles(events) == ["ESP32 valid-message timeout"]


def test_garbled_boot_after_panic_is_bounded_and_finalized_on_silence():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")
    capture_guru(monitor, now=1)
    monitor.observe_raw_line("rst:garbled boot output", 1.3)

    events = monitor.check_timeouts(3.4)

    assert titles(events) == ["ESP32 firmware panic detected"]
    assert "rst:garbled boot output" in events[0].context["panic_output"]
    assert monitor.check_timeouts(4) == []


def test_independent_panics_are_reported_after_esp_info_recovery():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")

    capture_guru(monitor, reason="LoadProhibited", now=1)
    monitor.observe_raw_line(BOOT, 1.3)
    first_events = monitor.observe_boot_reason("PANIC", "4", 2)
    assert monitor.observe_boot_reason("PANIC", "4", 2.1) == []
    assert monitor.observe_valid_message("ESPInfo", 2.2, "1.2.3") == []

    capture_guru(monitor, reason="StoreProhibited", now=3)
    monitor.observe_raw_line(BOOT, 3.3)
    second_events = monitor.observe_boot_reason("PANIC", "4", 4)

    assert titles(first_events) == ["ESP32 firmware panic detected"]
    assert titles(second_events) == ["ESP32 firmware panic detected"]
    assert [event.fingerprint for event in first_events + second_events] == [
        "esp32-firmware-panic-load-prohibited",
        "esp32-firmware-panic-store-prohibited",
    ]


def test_valid_traffic_finalizes_panic_and_clears_evidence_for_next_incident():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")
    capture_guru(monitor, reason="LoadProhibited", now=1)

    first_events = monitor.observe_valid_message("Data", 1.5)
    capture_guru(monitor, reason="StoreProhibited", now=2)
    second_events = monitor.observe_valid_message("Sensors", 2.5)

    assert [event.fingerprint for event in first_events + second_events] == [
        "esp32-firmware-panic-load-prohibited",
        "esp32-firmware-panic-store-prohibited",
    ]
    assert "LoadProhibited" in first_events[0].context["panic_output"]
    assert "LoadProhibited" not in second_events[0].context["panic_output"]


def test_begin_update_emits_pending_panic_before_clearing_state():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")
    capture_guru(monitor, now=1)

    events = monitor.begin_update("2.0.0", "1.2.3", 1.5)

    assert titles(events) == ["ESP32 firmware panic detected"]
    assert events[0].context["backtrace"].startswith("Backtrace:")
    assert monitor.phase == ESPCommunicationPhase.FLASHING


def test_older_firmware_valid_message_provides_one_unknown_reset_fallback():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")
    capture_guru(monitor)

    assert monitor.observe_raw_line(BOOT, 1.3) == []
    events = monitor.observe_valid_message("Data", 3)

    assert titles(events) == ["ESP32 firmware panic detected"]
    assert events[0].tags["reset_reason"] == "UNKNOWN"
    assert monitor.observe_valid_message("Sensors", 3.1) == []


def test_watchdog_reset_is_unexpected_reset_without_raw_backtrace():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")
    monitor.observe_raw_line(BOOT, 1)

    events = monitor.observe_boot_reason("TASK_WDT", "6", 3)

    assert titles(events) == ["ESP32 unexpected reset detected"]
    assert events[0].tags["reset_reason"] == "TASK_WDT"


def test_update_panic_is_deduplicated_across_recovery_boots():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "old")
    monitor.begin_update("new", "old", 1)
    monitor.finish_flashing(2)
    capture_guru(monitor, now=2.05)

    assert monitor.observe_raw_line(BOOT, 2.4) == []
    assert monitor.observe_raw_line(BOOT, 4) == []
    events = monitor.observe_boot_reason("PANIC", "4", 6)

    assert titles(events) == ["ESP32 firmware panic detected"]
    assert monitor.observe_valid_message("ESPBoot", 6.1) == []
    assert monitor.phase == ESPCommunicationPhase.WAITING_FOR_PROTOCOL


def test_normal_valid_message_timeout_is_deduplicated():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")

    assert monitor.check_timeouts(2) == []
    assert titles(monitor.check_timeouts(2.2)) == ["ESP32 valid-message timeout"]
    assert monitor.check_timeouts(5) == []


def test_expected_reset_timeout_is_distinct():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")
    monitor.begin_expected_reset(1)

    events = monitor.check_timeouts(16.1)

    assert titles(events) == ["ESP32 valid-message timeout"]
    assert events[0].tags["operation"] == "expected_reset"


def test_unexpected_boot_without_protocol_reaches_reset_recovery_timeout():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")

    assert monitor.observe_raw_line(BOOT, 1) == []
    assert monitor.phase == ESPCommunicationPhase.WAITING_FOR_PROTOCOL
    assert monitor.check_timeouts(16) == []

    events = monitor.check_timeouts(16.1)

    assert titles(events) == ["ESP32 valid-message timeout"]
    assert events[0].tags["operation"] == "unexpected_reset"
    assert events[0].context["threshold_seconds"] == 15.0
    assert monitor.check_timeouts(20) == []


@pytest.mark.parametrize("update_recovery", [False, True], ids=["reset", "update"])
@pytest.mark.parametrize(
    ("panic_lines", "core", "fingerprint"),
    [
        (
            [
                "Guru Meditation Error: Core 1 panic'ed (LoadProhibited).",
                "Core 1 register dump:",
                "Backtrace: 0x40381234:0x3fceabcd",
            ],
            "1",
            "esp32-firmware-panic-load-prohibited",
        ),
        (
            [
                "abort() was called at PC 0x420037e1 on core 0",
                "Register dump:",
                "Backtrace: 0x4037801a:0x3fcebce0",
            ],
            "0",
            "esp32-firmware-panic-abort",
        ),
    ],
    ids=["guru", "abort"],
)
def test_recovery_timeout_reports_retained_panic_once(
    update_recovery, panic_lines, core, fingerprint
):
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")
    if update_recovery:
        monitor.begin_update("2.0.0", "1.2.3", 1)
        monitor.finish_flashing(2)

    for index, line in enumerate(panic_lines):
        monitor.observe_raw_line(line, 2.1 + index / 10)
    assert monitor.observe_raw_line(BOOT, 3) == []

    timeout = 32.1 if update_recovery else 18.1
    events = monitor.check_timeouts(timeout)
    events += monitor.check_timeouts(timeout + 0.1)
    events += monitor.observe_boot_reason("PANIC", "4", timeout + 0.2)
    events += monitor.observe_valid_message("ESPBoot", timeout + 0.3)

    panic_events = [event for event in events if event.title == "ESP32 firmware panic detected"]
    assert len(panic_events) == 1
    panic = panic_events[0]
    assert panic.tags["reset_reason"] == "UNKNOWN"
    assert panic.context["reset_reason"] == "UNKNOWN"
    assert panic.context["core"] == core
    assert panic.context["previous_firmware"] == "1.2.3"
    assert panic.context["backtrace"].startswith("Backtrace:")
    assert len(panic.context["panic_output"].splitlines()) <= monitor.MAX_PANIC_LINES
    assert len(panic.context["panic_output"].encode()) <= monitor.MAX_PANIC_BYTES
    assert panic.fingerprint == fingerprint
    expected_recovery_title = (
        "ESP32 did not recover after firmware update"
        if update_recovery
        else "ESP32 valid-message timeout"
    )
    assert titles(events).count(expected_recovery_title) == 1

    capture_guru(monitor, reason="StoreProhibited", now=timeout + 1)
    monitor.observe_raw_line(BOOT, timeout + 1.3)
    new_events = monitor.observe_boot_reason("PANIC", "4", timeout + 1.4)
    assert [event.fingerprint for event in new_events] == [
        "esp32-firmware-panic-store-prohibited"
    ]


def test_unexpected_boot_protocol_message_clears_recovery_timeout():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")
    monitor.observe_raw_line(BOOT, 1)

    events = monitor.observe_boot_reason("SW", "3", 2)
    assert monitor.observe_valid_message("ESPBoot", 2) == []

    assert titles(events) == ["ESP32 unexpected reset detected"]
    assert monitor.phase == ESPCommunicationPhase.NORMAL
    assert monitor.recovery_deadline is None
    assert monitor.check_timeouts(4) == []
    assert titles(monitor.check_timeouts(4.1)) == ["ESP32 valid-message timeout"]


def test_unexpected_boot_loop_is_detected_without_protocol_follow_up():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")

    events = []
    for timestamp in (1, 10, 20):
        events += monitor.observe_raw_line(BOOT, timestamp)

    assert titles(events) == ["ESP32 firmware boot loop detected"]


def test_three_unexpected_boots_report_boot_loop_once():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")
    events = []
    for timestamp in (1, 10, 20, 30):
        events += monitor.observe_raw_line(BOOT, timestamp)
        monitor.observe_boot_reason("SW", "3", timestamp + 0.1)
        monitor.observe_valid_message("ESPInfo", timestamp + 0.2, "1.2.3")

    assert titles(events) == ["ESP32 firmware boot loop detected"]


def test_panic_evidence_is_bounded_by_line_count():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")
    monitor.observe_raw_line("Guru Meditation Error: Core 1 panic'ed (LoadProhibited).", 1)
    for index in range(100):
        monitor.observe_raw_line(f"register {index}", 1.1 + index / 100)
    monitor.observe_raw_line(BOOT, 3)

    event = monitor.observe_boot_reason("PANIC", "4", 4)[0]

    assert len(event.context["panic_output"].splitlines()) == 80
    assert event.context["panic_output_truncated"] is True


def test_panic_evidence_is_bounded_by_byte_count():
    monitor = ESPObservability(now=0)
    monitor.observe_raw_line("abort() was called at PC 0x1 on core 0", 1)
    for index in range(20):
        monitor.observe_raw_line(f"{index}:" + "x" * 1000, 1.1 + index / 100)
    monitor.observe_raw_line(BOOT, 3)

    event = monitor.observe_boot_reason("PANIC", "4", 4)[0]

    assert len(event.context["panic_output"].encode()) <= 8192
    assert event.context["panic_output_truncated"] is True


def test_known_panic_causes_have_separate_fingerprints():
    load = ESPObservability()
    capture_guru(load, "LoadProhibited")
    load.observe_raw_line(BOOT, 2)
    load_event = load.observe_boot_reason("PANIC", "4", 3)[0]

    abort = ESPObservability()
    abort.observe_raw_line("abort() was called at PC 0x1 on core 0", 1)
    abort.observe_raw_line(BOOT, 2)
    abort_event = abort.observe_boot_reason("PANIC", "4", 3)[0]

    assert load_event.fingerprint == "esp32-firmware-panic-load-prohibited"
    assert abort_event.fingerprint == "esp32-firmware-panic-abort"


@pytest.mark.parametrize(
    ("line", "fingerprint"),
    [
        ("assert failed: x.cpp:42", "esp32-firmware-panic-assert"),
        (
            "Stack smashing protect failure!",
            "esp32-firmware-panic-stack-smashing",
        ),
        ("Heap corruption detected", "esp32-firmware-panic-heap-corruption"),
    ],
)
def test_other_primary_panic_lines_have_normalized_fingerprints(line, fingerprint):
    monitor = ESPObservability(now=0)
    monitor.observe_raw_line(line, 1)

    event = monitor.check_timeouts(3.1)[0]

    assert event.fingerprint == fingerprint


def test_variable_unknown_panic_causes_share_bounded_fingerprint():
    fingerprints = set()
    for reason in ("Task foo at 0x1234", "Task bar at 0x9876"):
        monitor = ESPObservability()
        capture_guru(monitor, reason)
        monitor.observe_raw_line(BOOT, 2)
        fingerprints.add(monitor.observe_boot_reason("PANIC", "4", 3)[0].fingerprint)

    assert fingerprints == {"esp32-firmware-panic-unknown"}
