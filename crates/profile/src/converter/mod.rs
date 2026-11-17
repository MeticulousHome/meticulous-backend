//! The parts of `profile_converter/` that `manual_program.py` reaches.
//!
//! The Python converter turns a simplified profile into the node program the
//! ESP32 runs; only the manual-mode path uses it now, and only for the head
//! and tail templates, node and reference id allocation, and a handful of
//! node, controller and trigger builders. Those are ported here with the
//! Python module layout; the rest (`to_complex`, `HeadProfile`,
//! `TailTemplate`, `profile_json`, `references`, `stages`, `curve`) is not
//! reachable and is not ported.
//!
//! Every builder produces its JSON object with the keys in the order the
//! Python dict gets them, because the program is serialised with
//! `json.dumps` and hashed.

pub mod controllers;
pub mod dictionaries;
pub mod nodes;
pub mod profile_converter;
pub mod simplified_json;
pub mod triggers;

pub use profile_converter::ComplexProfileConverter;
pub use simplified_json::{InitNode, ReferenceIds, SimplifiedJson};

/// `config.MAX_PISTON_POSITION`: the piston travel in mm, a constant.
pub const MAX_PISTON_POSITION: i64 = 75;
