//! Output recorded from CPython 3.12 (`repr(float)`, `json.dumps`): random
//! doubles across the whole range plus values shaped like sensor readings,
//! and documents with every kind of character `json.dumps` escapes.

use pyfmt::{DumpOpts, dumps, repr_f64};
use serde_json::Value;

#[test]
fn float_repr_matches_cpython() {
    let data = include_str!("data/float_repr.txt");
    let mut checked = 0;
    for line in data.lines() {
        let (bits, expected) = line.split_once(' ').expect("golden line");
        let value = f64::from_bits(u64::from_str_radix(bits, 16).expect("hex bits"));
        assert_eq!(repr_f64(value), expected, "repr of {bits}");
        checked += 1;
    }
    assert!(checked > 9000);
}

#[test]
fn json_dumps_matches_cpython() {
    let docs: Vec<Value> =
        serde_json::from_str(include_str!("data/dumps_goldens.json")).expect("goldens");
    let sorted = DumpOpts {
        sort_keys: true,
        ..DumpOpts::default()
    };
    for entry in &docs {
        let doc = &entry["doc"];
        assert_eq!(
            dumps(doc, &DumpOpts::default()),
            entry["default"].as_str().unwrap_or("")
        );
        assert_eq!(
            dumps(doc, &DumpOpts::indented(4)),
            entry["indent4"].as_str().unwrap_or("")
        );
        assert_eq!(
            dumps(doc, &DumpOpts::unicode()),
            entry["unicode"].as_str().unwrap_or("")
        );
        assert_eq!(dumps(doc, &sorted), entry["sorted"].as_str().unwrap_or(""));
    }
}
