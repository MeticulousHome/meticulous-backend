//! CPython-compatible formatting and parsing.
//!
//! The Python backend's observable output is defined by CPython: the bytes it
//! writes to the ESP32 come from `str(float)` and `json.dumps`, the profile
//! files it stores come from `json.dump(..., indent=4)`, and it reads the
//! ESP32's text with `float()`, `int()` and `urllib.parse.unquote`. Machines
//! keep those files across updates and the firmware hashes the JSON it
//! receives, so the Rust backend reproduces these exactly. Each function names
//! the CPython behaviour it mirrors.

mod float;
mod json;
mod text;

pub use float::{PyFloat, py_float, py_int, repr_f64, safe_float, safe_float_with_nan};
pub use json::{DumpOpts, dumps, dumps_default, loads_lenient, object, tornado_json_encode};
pub use text::{strip_crlf, unquote, unquote_to_bytes};
