//! The ESP32 side of the Meticulous backend, without IO.
//!
//! - [`data`] holds the messages of `esp_serial/data.py`, parsed from their
//!   fields and rendered back (`to_args` for the emulator, `to_sio` for
//!   Socket.IO);
//! - [`line`] is the line dispatch of `Machine._read_data` (`machine.py`);
//! - [`encode`] writes every message the backend sends to the ESP32;
//! - [`sync`] takes the settings and device UUID handshake decisions of
//!   `Machine`;
//! - [`observability`] is `esp_observability.py`;
//! - [`motor`] is `monitoring/motor_power_monitoring.py`;
//! - [`version`] parses firmware versions (`Machine._parseVersionString`,
//!   `packaging.version.Version`, the version stamp in a firmware image).
//!
//! The Python sources are the reference: field fallbacks, number types and
//! failure cases follow them, and Python quirks a client could see are kept
//! and marked `PARITY:`.

pub mod data;
pub mod encode;
pub mod line;
pub mod motor;
pub mod observability;
mod pynum;
pub mod sync;
pub mod version;

pub use data::{
    ButtonEvent, ButtonEventData, ControlTypes, EspInfo, HeaterTimeoutError, HeaterTimeoutInfo,
    MachineNotify, MachineState, MachineStatus, SensorData, ShotData, ShotDataTooShort,
    is_valid_device_uuid, machine_status_to_profile,
};
pub use line::{
    ClassifiedLine, EspLine, EspLog, LogWithoutMessage, Uncaught, classify_line, decode_line,
    is_boot_banner, split_fields,
};
pub use motor::EnergyCalculator;
pub use observability::{
    DiagnosticAlarm, EspCommunicationPhase, EspDiagnostic, EspObservability,
    should_start_firmware_update,
};
pub use pynum::PyNum;
pub use version::{
    ExtraCommits, FirmwareVersion, Pep440Version, firmware_image_app_version, parse_version_string,
};
