//! Espresso and Pour Over profiles, without IO.
//!
//! Ports `profile_preprocessor.py` (variables), `manual_program.py` and the
//! parts of `profile_converter/` it reaches (the manual-mode node program),
//! the pure part of `manual_mode.py` (a profile from a recorded manual
//! brew), the pure helpers of `profiles.py` (validation, what goes to the
//! ESP32, the Manual mode profile, normalisation on load) and the validation
//! of `pour_over_profiles.py`, with the Python's observable behaviour:
//! exception class names and messages that reach clients, `jsonschema`'s
//! `best_match` messages, and node programs that serialise
//! (`pyfmt::dumps_default`) to the bytes the Python sends.
//!
//! Python bugs that clients can observe are kept and marked `PARITY:`.

pub mod converter;
pub mod manual_mode;
pub mod manual_program;
pub mod pour_over;
pub mod preprocessor;
pub mod profiles;
pub mod pynum;
pub mod pyops;
pub mod pyrepr;
pub mod pyround;
pub mod schema;

pub use converter::ReferenceIds;
pub use manual_mode::{
    LocalDateTime, ManualShotEnv, ManualShotError, build_profile_from_manual_shot,
    manual_shot_search_ids, pick_manual_shot,
};
pub use manual_program::{ProgramError, ProgramSettings, build_manual_program, validate_program};
pub use pour_over::{
    MAX_POUR_OVER_PROFILE_BYTES, PourOverIssue, PourOverProfileError, validate_pour_over_profile,
};
pub use preprocessor::{PreprocessError, process_variables};
pub use profiles::{
    ESP32_PROFILE_FIELDS, ManualModeSeed, ProfileError, build_manual_mode_profile,
    manual_mode_stages, manual_profile_has_seeded_stages, normalize_loaded_profile,
    plan_manual_mode_profile, prepare_for_esp32, profile_for_esp32, validate_profile,
};
pub use schema::{ValidationError, Validator};
