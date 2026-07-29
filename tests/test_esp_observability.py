from esp_observability import ESPCommunicationPhase, ESPObservability


BOOT = "rst:0xc (RTC_SW_CPU_RST),boot:0x8 (SPI_FAST_FLASH_BOOT)"


def titles(events):
    return [event.title for event in events]


def test_stale_pre_update_info_does_not_finish_update():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", now=0.1, firmware_version="old")
    monitor.begin_update("new", "old", now=1)
    monitor.finish_flashing(now=10)

    assert monitor.observe_valid_message("ESPInfo", 10.1, "old") == []
    assert monitor.phase == ESPCommunicationPhase.WAITING_FOR_BOOT
    assert monitor.check_timeouts(12.5) == []


def test_update_requires_boot_and_expected_esp_info_without_false_timeout():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "old")
    monitor.begin_update("new", "old", 1)
    monitor.finish_flashing(10)

    assert monitor.observe_raw_line(BOOT, 10.2) == []
    assert monitor.phase == ESPCommunicationPhase.WAITING_FOR_PROTOCOL
    assert monitor.check_timeouts(15) == []
    assert monitor.observe_boot_reason("SW", "3", 12) == []
    assert monitor.observe_valid_message("Data", 12.1) == []
    assert monitor.phase == ESPCommunicationPhase.WAITING_FOR_PROTOCOL
    assert monitor.observe_valid_message("ESPInfo", 12.2, "new") == []
    assert monitor.phase == ESPCommunicationPhase.NORMAL


def test_update_recovery_failure_is_specific():
    monitor = ESPObservability(now=0)
    monitor.begin_update("new", "old", 1)
    monitor.finish_flashing(2)

    events = monitor.check_timeouts(32.1)

    assert titles(events) == ["ESP32 did not recover after firmware update"]
    assert events[0].tags["recovery_phase"] == "waiting_for_boot"


def test_flash_error_is_specific_and_restores_normal_state():
    monitor = ESPObservability(now=0)
    monitor.begin_update("new", "old", 1)

    event = monitor.fail_flashing("esptool failed", "flash", 2)

    assert event.title == "ESP32 firmware update failed"
    assert monitor.phase == ESPCommunicationPhase.NORMAL


def test_expected_api_reset_emits_no_error():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "current")
    monitor.begin_expected_reset(1)

    assert monitor.observe_raw_line(BOOT, 1.2) == []
    assert monitor.observe_boot_reason("SW", "3", 3) == []
    assert monitor.observe_valid_message("ESPInfo", 3.1, "current") == []
    assert monitor.phase == ESPCommunicationPhase.NORMAL


def test_guru_meditation_is_reported_as_panic_with_bounded_evidence():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")
    monitor.observe_raw_line("Guru Meditation Error: Core  1 panic'ed (LoadProhibited).", 1)
    monitor.observe_raw_line("Core  1 register dump:", 1.1)
    monitor.observe_raw_line("Backtrace: 0x40381234:0x3fceabcd", 1.2)
    events = monitor.observe_raw_line(BOOT, 1.3)

    assert monitor.observe_boot_reason("PANIC", "4", 3.5) == []

    assert titles(events) == ["ESP32 firmware panic detected"]
    assert events[0].tags["reset_reason"] == "UNKNOWN"
    assert events[0].tags["panic_reason"] == "LoadProhibited"
    assert events[0].tags["core"] == "1"
    assert "Guru Meditation Error" in events[0].context["panic_output"]
    assert events[0].context["previous_firmware"] == "1.2.3"


def test_abort_panic_extracts_core_and_keeps_the_cause_line():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")
    monitor.observe_raw_line("abort() was called at PC 0x420037e1 on core 1", 1)
    monitor.observe_raw_line("Backtrace: 0x4037801a:0x3fcebce0", 1.1)

    events = monitor.observe_raw_line(BOOT, 1.2)

    assert titles(events) == ["ESP32 firmware panic detected"]
    assert events[0].tags["panic_reason"] == "abort"
    assert events[0].tags["core"] == "1"
    assert events[0].context["panic_output"].startswith("abort() was called")


def test_watchdog_reset_is_classified_as_unexpected_reset_without_raw_backtrace():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")
    monitor.observe_raw_line(BOOT, 1)

    events = monitor.observe_boot_reason("TASK_WDT", "6", 3)

    assert titles(events) == ["ESP32 unexpected reset detected"]
    assert events[0].tags["reset_reason"] == "TASK_WDT"


def test_expected_update_still_reports_a_real_panic():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "old")
    monitor.begin_update("new", "old", 1)
    monitor.finish_flashing(2)
    monitor.observe_raw_line("Guru Meditation Error: Core 1 panic'ed.", 2.05)

    events = monitor.observe_raw_line(BOOT, 2.1)

    assert titles(events) == ["ESP32 firmware panic detected"]
    assert monitor.observe_boot_reason("PANIC", "4", 4) == []
    assert monitor.phase == ESPCommunicationPhase.WAITING_FOR_PROTOCOL


def test_second_boot_during_update_does_not_duplicate_panic():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "old")
    monitor.begin_update("new", "old", 1)
    monitor.finish_flashing(2)
    monitor.observe_raw_line("Backtrace: 0x40381234:0x3fceabcd", 2.05)

    first_boot_events = monitor.observe_raw_line(BOOT, 2.1)
    second_boot_events = monitor.observe_raw_line(BOOT, 4)
    reason_events = monitor.observe_boot_reason("PANIC", "4", 6)
    protocol_events = monitor.observe_valid_message("ESPBoot", 6.1)

    assert titles(first_boot_events) == ["ESP32 firmware panic detected"]
    assert second_boot_events == []
    assert reason_events == []
    assert protocol_events == []


def test_non_panic_reset_is_not_called_a_crash():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")
    monitor.observe_raw_line(BOOT, 1)

    events = monitor.observe_boot_reason("BROWNOUT", "9", 3)

    assert titles(events) == ["ESP32 unexpected reset detected"]
    assert events[0].tags["reset_reason"] == "BROWNOUT"


def test_parser_valid_message_timeout_uses_normal_operation_threshold():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")

    assert monitor.check_timeouts(2) == []
    events = monitor.check_timeouts(2.2)
    assert titles(events) == ["ESP32 valid-message timeout"]
    assert monitor.check_timeouts(5) == []


def test_expected_reset_that_never_recovers_becomes_valid_message_timeout():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")
    monitor.begin_expected_reset(1)

    events = monitor.check_timeouts(16.1)

    assert titles(events) == ["ESP32 valid-message timeout"]
    assert events[0].tags["operation"] == "expected_reset"


def test_three_unexpected_boots_report_boot_loop_once():
    monitor = ESPObservability(now=0)
    monitor.observe_valid_message("ESPInfo", 0.1, "1.2.3")
    events = []
    for timestamp in (1, 10, 20):
        events += monitor.observe_raw_line(BOOT, timestamp)
        monitor.observe_boot_reason("SW", "3", timestamp + 0.1)
        monitor.observe_valid_message("ESPInfo", timestamp + 0.2, "1.2.3")

    assert titles(events) == ["ESP32 firmware boot loop detected"]
