from collections import deque
from dataclasses import dataclass, field
from enum import Enum
import re


class ESPCommunicationPhase(Enum):
    NORMAL = "normal"
    EXPECTED_RESET = "expected_reset"
    FLASHING = "flashing"
    WAITING_FOR_BOOT = "waiting_for_boot"
    WAITING_FOR_PROTOCOL = "waiting_for_protocol"


@dataclass(frozen=True)
class ESPDiagnostic:
    title: str
    level: str
    fingerprint: str
    tags: dict[str, str] = field(default_factory=dict)
    context: dict[str, object] = field(default_factory=dict)


class ESPObservability:
    VALID_MESSAGE_TIMEOUT_SECONDS = 2.0
    UPDATE_RECOVERY_TIMEOUT_SECONDS = 30.0
    RESET_RECOVERY_TIMEOUT_SECONDS = 15.0
    BOOT_LOOP_WINDOW_SECONDS = 60.0
    BOOT_LOOP_THRESHOLD = 3
    MAX_PANIC_LINES = 80
    MAX_PANIC_BYTES = 8192

    PANIC_MARKERS = (
        "guru meditation error",
        "abort() was called",
        "register dump",
        "backtrace",
        "assert failed",
        "stack smashing protect failure",
        "heap corruption",
    )
    GURU_MEDITATION_PATTERN = re.compile(
        r"Guru Meditation Error:\s*Core\s+(\d+)\s+panic'ed(?:\s+\(([^)]+)\))?",
        re.IGNORECASE,
    )
    ABORT_PATTERN = re.compile(
        r"abort\(\) was called at PC\s+\S+\s+on core\s+(\d+)",
        re.IGNORECASE,
    )
    DEBUG_EXCEPTION_PATTERN = re.compile(r"Debug exception reason:\s*(.+)", re.IGNORECASE)
    PANIC_CAUSE_ALIASES = {
        "abort": "abort",
        "stack canary watchpoint triggered": "stack-canary",
        "loadprohibited": "load-prohibited",
        "storeprohibited": "store-prohibited",
        "illegalinstruction": "illegal-instruction",
        "integerdividebyzero": "integer-divide-by-zero",
        "cache error": "cache-error",
        "interrupt wdt timeout on cpu0": "interrupt-watchdog",
        "interrupt wdt timeout on cpu1": "interrupt-watchdog",
        "task watchdog got triggered": "task-watchdog",
        "stack smashing protect failure": "stack-smashing",
        "heap corruption": "heap-corruption",
        "assert failed": "assert",
    }

    def __init__(self, now: float = 0.0):
        self.phase = ESPCommunicationPhase.EXPECTED_RESET
        self.last_valid_message = now
        self.recovery_deadline = now + self.RESET_RECOVERY_TIMEOUT_SECONDS
        self.expected_firmware = None
        self._update_active = False
        self.previous_firmware = None
        self.boot_seen = False
        self.pending_unexpected_reset = False
        self.reset_reported = False
        self.panic_reported = False
        self.timeout_reported = False
        self._collecting_panic = False
        self._panic_lines: list[str] = []
        self._panic_bytes = 0
        self._panic_core = None
        self._panic_reason = None
        self._exception_reason = None
        self._panic_location = None
        self._recent_unexpected_boots: deque[float] = deque()

    @property
    def update_in_progress(self) -> bool:
        return self._update_active and self.phase in {
            ESPCommunicationPhase.FLASHING,
            ESPCommunicationPhase.WAITING_FOR_BOOT,
            ESPCommunicationPhase.WAITING_FOR_PROTOCOL,
        }

    def begin_expected_reset(self, now: float) -> None:
        self.phase = ESPCommunicationPhase.EXPECTED_RESET
        self._update_active = False
        self.boot_seen = False
        self.recovery_deadline = now + self.RESET_RECOVERY_TIMEOUT_SECONDS
        self.pending_unexpected_reset = False
        self.reset_reported = False
        self.panic_reported = False
        self.timeout_reported = False

    def begin_update(
        self, expected_firmware: str | None, previous_firmware: str | None, now: float
    ) -> None:
        self.phase = ESPCommunicationPhase.FLASHING
        self._update_active = True
        self.expected_firmware = self._clean_version(expected_firmware)
        self.previous_firmware = self._clean_version(previous_firmware)
        self.boot_seen = False
        self.recovery_deadline = None
        self.pending_unexpected_reset = False
        self.reset_reported = False
        self.panic_reported = False
        self.timeout_reported = False
        self._clear_panic()

    def finish_flashing(self, now: float) -> None:
        self.phase = ESPCommunicationPhase.WAITING_FOR_BOOT
        self.recovery_deadline = now + self.UPDATE_RECOVERY_TIMEOUT_SECONDS

    def fail_flashing(self, detail: str, stage: str, now: float) -> ESPDiagnostic:
        event = ESPDiagnostic(
            title="ESP32 firmware update failed",
            level="error",
            fingerprint="esp32-firmware-update-failed",
            tags={"stage": stage},
            context={
                "detail": detail[:1000],
                "expected_firmware": self.expected_firmware,
                "previous_firmware": self.previous_firmware,
            },
        )
        self._return_to_normal(now)
        return event

    def observe_raw_line(self, line: str, now: float) -> list[ESPDiagnostic]:
        events: list[ESPDiagnostic] = []
        normalized = line.strip("\r\n")
        lowered = normalized.lower()
        is_boot_banner = (
            normalized.startswith("rst:0x")
            and "boot:0x" in normalized
            and " (SPI_FAST_FLASH_BOOT)" in normalized
        )

        if any(marker in lowered for marker in self.PANIC_MARKERS):
            self._collecting_panic = True
            guru_match = self.GURU_MEDITATION_PATTERN.search(normalized)
            if guru_match:
                self._panic_core = guru_match.group(1)
                self._panic_reason = guru_match.group(2)
            abort_match = self.ABORT_PATTERN.search(normalized)
            if abort_match:
                self._panic_core = abort_match.group(1)
                self._panic_reason = "abort"

        debug_match = self.DEBUG_EXCEPTION_PATTERN.search(normalized)
        if debug_match:
            detail = debug_match.group(1).strip().rstrip(".")
            location_match = re.fullmatch(r"(.+?)\s*\(([^()]*)\)", detail)
            if location_match:
                self._exception_reason = location_match.group(1).strip()
                self._panic_location = location_match.group(2).strip() or None
            else:
                self._exception_reason = detail

        if self._collecting_panic and not is_boot_banner:
            self._append_panic_line(normalized)

        if not is_boot_banner:
            return events

        self.boot_seen = True
        self._collecting_panic = False
        if self.update_in_progress:
            self.phase = ESPCommunicationPhase.WAITING_FOR_PROTOCOL
            return events

        if self.phase in {
            ESPCommunicationPhase.EXPECTED_RESET,
            ESPCommunicationPhase.WAITING_FOR_PROTOCOL,
        }:
            self.phase = ESPCommunicationPhase.WAITING_FOR_PROTOCOL
            return events

        self.pending_unexpected_reset = True
        self.reset_reported = False
        self._recent_unexpected_boots.append(now)
        cutoff = now - self.BOOT_LOOP_WINDOW_SECONDS
        while self._recent_unexpected_boots and self._recent_unexpected_boots[0] < cutoff:
            self._recent_unexpected_boots.popleft()
        if len(self._recent_unexpected_boots) == self.BOOT_LOOP_THRESHOLD:
            events.append(
                ESPDiagnostic(
                    title="ESP32 firmware boot loop detected",
                    level="critical",
                    fingerprint="esp32-firmware-boot-loop",
                    context={
                        "resets": len(self._recent_unexpected_boots),
                        "window_seconds": self.BOOT_LOOP_WINDOW_SECONDS,
                        "previous_firmware": self.previous_firmware,
                    },
                )
            )
        return events

    def observe_boot_reason(self, reason: str, code: str, now: float) -> list[ESPDiagnostic]:
        reason = reason.strip().upper() or "UNKNOWN"
        code = code.strip()
        if self.panic_reported:
            return []
        if reason == "PANIC" or self._panic_lines:
            self.panic_reported = True
            self.reset_reported = True
            return [self._panic_diagnostic(reason, code)]
        if not self.pending_unexpected_reset or self.reset_reported:
            return []

        self.reset_reported = True
        return [
            ESPDiagnostic(
                title="ESP32 unexpected reset detected",
                level="error",
                fingerprint=f"esp32-unexpected-reset-{reason.lower()}",
                tags={"reset_reason": reason, "reset_reason_code": code},
                context={"previous_firmware": self.previous_firmware},
            )
        ]

    def observe_valid_message(
        self,
        message_type: str,
        now: float,
        firmware_version: str | None = None,
    ) -> list[ESPDiagnostic]:
        self.last_valid_message = now
        self.timeout_reported = False

        if self._panic_lines and not self.panic_reported:
            self.panic_reported = True
            self.reset_reported = True
            events = [self._panic_diagnostic("UNKNOWN", "")]
        elif self.pending_unexpected_reset and not self.reset_reported:
            self.reset_reported = True
            events = [
                ESPDiagnostic(
                    title="ESP32 unexpected reset detected",
                    level="error",
                    fingerprint="esp32-unexpected-reset-unknown",
                    tags={"reset_reason": "UNKNOWN"},
                    context={"previous_firmware": self.previous_firmware},
                )
            ]
        else:
            events = []

        if message_type != "ESPInfo":
            return events

        observed_firmware = self._clean_version(firmware_version)
        if self.update_in_progress:
            if not self.boot_seen:
                return events
            if self.expected_firmware is None or observed_firmware != self.expected_firmware:
                return events
            self._return_to_normal(now)
        elif self.phase in {
            ESPCommunicationPhase.EXPECTED_RESET,
            ESPCommunicationPhase.WAITING_FOR_PROTOCOL,
        }:
            self._return_to_normal(now)
        elif self.pending_unexpected_reset:
            self.pending_unexpected_reset = False
            self._clear_panic()

        if observed_firmware:
            self.previous_firmware = observed_firmware
        return events

    def check_timeouts(self, now: float) -> list[ESPDiagnostic]:
        if (
            self.update_in_progress
            and self.recovery_deadline is not None
            and now > self.recovery_deadline
        ):
            phase = self.phase.value
            event = ESPDiagnostic(
                title="ESP32 did not recover after firmware update",
                level="critical",
                fingerprint="esp32-firmware-update-recovery-failed",
                tags={"recovery_phase": phase},
                context={
                    "expected_firmware": self.expected_firmware,
                    "previous_firmware": self.previous_firmware,
                    "timeout_seconds": self.UPDATE_RECOVERY_TIMEOUT_SECONDS,
                },
            )
            self._return_to_normal(now)
            self.timeout_reported = True
            return [event]

        if (
            not self._update_active
            and self.phase
            in {
                ESPCommunicationPhase.EXPECTED_RESET,
                ESPCommunicationPhase.WAITING_FOR_PROTOCOL,
            }
            and self.recovery_deadline is not None
            and now > self.recovery_deadline
        ):
            phase = self.phase.value
            self._return_to_normal(now)
            self.timeout_reported = True
            return [
                ESPDiagnostic(
                    title="ESP32 valid-message timeout",
                    level="error",
                    fingerprint="esp32-valid-message-timeout",
                    tags={"operation": "expected_reset"},
                    context={
                        "recovery_phase": phase,
                        "threshold_seconds": self.RESET_RECOVERY_TIMEOUT_SECONDS,
                        "last_known_firmware": self.previous_firmware,
                    },
                )
            ]

        if self.phase != ESPCommunicationPhase.NORMAL or self.pending_unexpected_reset:
            return []

        elapsed = now - self.last_valid_message
        if elapsed > self.VALID_MESSAGE_TIMEOUT_SECONDS and not self.timeout_reported:
            self.timeout_reported = True
            return [
                ESPDiagnostic(
                    title="ESP32 valid-message timeout",
                    level="error",
                    fingerprint="esp32-valid-message-timeout",
                    context={
                        "elapsed_seconds": round(elapsed, 3),
                        "threshold_seconds": self.VALID_MESSAGE_TIMEOUT_SECONDS,
                        "last_known_firmware": self.previous_firmware,
                    },
                )
            ]
        return []

    def _panic_diagnostic(self, reason: str, code: str) -> ESPDiagnostic:
        tags = {"reset_reason": reason, "reset_reason_code": code}
        context: dict[str, object] = {
            "reset_reason": reason,
            "previous_firmware": self.previous_firmware,
        }
        structured_fields = {
            "panic_reason": self._panic_reason,
            "exception_reason": self._exception_reason,
            "panic_location": self._panic_location,
            "core": self._panic_core,
        }
        for key, value in structured_fields.items():
            if value:
                context[key] = value
                if key != "panic_location":
                    tags[key] = value
        if code:
            context["reset_reason_code"] = code
        if self._panic_lines:
            context["panic_output"] = "\n".join(self._panic_lines)
            context["backtrace"] = "\n".join(
                line for line in self._panic_lines if "backtrace" in line.lower()
            )
            context["panic_output_truncated"] = (
                len(self._panic_lines) >= self.MAX_PANIC_LINES
                or self._panic_bytes >= self.MAX_PANIC_BYTES
            )
        cause = self._normalized_panic_cause()
        return ESPDiagnostic(
            title="ESP32 firmware panic detected",
            level="critical",
            fingerprint=f"esp32-firmware-panic-{cause}",
            tags=tags,
            context=context,
        )

    def _normalized_panic_cause(self) -> str:
        candidates = (self._exception_reason, self._panic_reason)
        for candidate in candidates:
            if not candidate:
                continue
            normalized = candidate.strip().lower().rstrip(".")
            cause = self.PANIC_CAUSE_ALIASES.get(normalized)
            if cause:
                return cause
        return "unknown"

    def _append_panic_line(self, line: str) -> None:
        if len(self._panic_lines) >= self.MAX_PANIC_LINES:
            return
        encoded_size = len(line.encode("utf-8", errors="replace")) + 1
        remaining = self.MAX_PANIC_BYTES - self._panic_bytes
        if remaining <= 0:
            return
        if encoded_size > remaining:
            line = line.encode("utf-8", errors="replace")[:remaining].decode(
                "utf-8", errors="ignore"
            )
            encoded_size = len(line.encode("utf-8", errors="replace"))
        self._panic_lines.append(line)
        self._panic_bytes += encoded_size

    def _return_to_normal(self, now: float) -> None:
        self.phase = ESPCommunicationPhase.NORMAL
        self.last_valid_message = now
        self.recovery_deadline = None
        self.expected_firmware = None
        self._update_active = False
        self.boot_seen = False
        self.pending_unexpected_reset = False
        self.reset_reported = False
        self.panic_reported = False
        self._clear_panic()

    def _clear_panic(self) -> None:
        self._collecting_panic = False
        self._panic_lines = []
        self._panic_bytes = 0
        self._panic_core = None
        self._panic_reason = None
        self._exception_reason = None
        self._panic_location = None

    @staticmethod
    def _clean_version(version: str | None) -> str | None:
        if version is None:
            return None
        cleaned = version.strip()
        return cleaned or None
