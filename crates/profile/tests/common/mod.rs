//! Helpers shared by the integration tests.

#![allow(dead_code)]

use std::path::PathBuf;

use md5::{Digest, Md5};
use profile::manual_mode::{LocalDateTime, ManualShotEnv};
use serde_json::Value;

pub fn data_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join(name)
}

/// A golden file written by `tools/gen_goldens.py`.
pub fn golden(name: &str) -> Value {
    let path = data_path(name);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

/// A file of the repository the crate lives in (the Python backend's tree).
pub fn repo_file(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join(relative)
}

pub fn repo_json(relative: &str) -> Value {
    let path = repo_file(relative);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "read {}: {e} (is the profile_schema submodule checked out? \
             `git submodule update --init profile_schema`)",
            path.display()
        )
    });
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

pub fn md5_hex(text: &str) -> String {
    let digest = Md5::digest(text.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// `json.dumps(value)`.
pub fn dumps(value: &Value) -> String {
    pyfmt::dumps_default(value)
}

/// `base64.b64encode(data)`.
pub fn b64encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// `"data:image/jpeg;base64," + b64encode(b"\xff\xd8" + zeros + b"\xff\xd9")`.
pub fn jpeg_data_uri(size: usize) -> String {
    let mut body = vec![0xff, 0xd8];
    body.extend(std::iter::repeat_n(0u8, size.saturating_sub(4)));
    body.extend([0xff, 0xd9]);
    format!("data:image/jpeg;base64,{}", b64encode(&body))
}

fn step<'a>(value: &'a mut Value, key: &Value) -> &'a mut Value {
    match (value, key) {
        (Value::Object(map), Value::String(k)) => map.get_mut(k).expect("path key exists"),
        (Value::Array(items), Value::Number(i)) => {
            let index = i.as_u64().expect("index") as usize;
            &mut items[index]
        }
        (other, key) => panic!("bad path step {key} into {other}"),
    }
}

/// The edit operations of `gen_goldens.apply_ops`.
pub fn apply_ops(base: &Value, ops: &Value) -> Value {
    let mut doc = base.clone();
    for op in ops.as_array().expect("ops list") {
        let path = op["path"].as_array().expect("path");
        let kind = op["op"].as_str().expect("op");
        if path.is_empty() {
            assert_eq!(kind, "set");
            doc = op["value"].clone();
            continue;
        }
        let mut parent = &mut doc;
        for key in &path[..path.len() - 1] {
            parent = step(parent, key);
        }
        let last = &path[path.len() - 1];
        match (kind, parent, last) {
            ("set", Value::Object(map), Value::String(k)) => {
                map.insert(k.clone(), op["value"].clone());
            }
            ("set", Value::Array(items), Value::Number(i)) => {
                items[i.as_u64().expect("index") as usize] = op["value"].clone();
            }
            ("del", Value::Object(map), Value::String(k)) => {
                map.shift_remove(k);
            }
            ("del", Value::Array(items), Value::Number(i)) => {
                items.remove(i.as_u64().expect("index") as usize);
            }
            ("insert", Value::Array(items), Value::Number(i)) => {
                let index = (i.as_u64().expect("index") as usize).min(items.len());
                items.insert(index, op["value"].clone());
            }
            ("repeat", parent, last) => {
                let target = step(parent, last);
                let first = target[0].clone();
                let count = op["count"].as_u64().expect("count") as usize;
                *target = Value::Array(vec![first; count]);
            }
            ("jpeg", parent, last) => {
                let size = op["size"].as_u64().expect("size") as usize;
                *step(parent, last) = Value::String(jpeg_data_uri(size));
            }
            (kind, parent, last) => panic!("cannot apply {kind} at {last} in {parent}"),
        }
    }
    doc
}

/// `str(uuid.UUID(int=n))`.
pub fn counter_uuid(n: u128) -> String {
    let hex = format!("{n:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// uuid4 replaced by a counter (as the generator does) and the clock in UTC.
#[derive(Default)]
pub struct TestEnv {
    pub uuids: u128,
}

impl ManualShotEnv for TestEnv {
    fn new_uuid(&mut self) -> String {
        self.uuids += 1;
        counter_uuid(self.uuids)
    }

    fn local_time(&self, timestamp: f64) -> Result<LocalDateTime, (String, String)> {
        // Microseconds rounded half to even, as datetime.fromtimestamp does.
        let mut seconds = timestamp.floor();
        let micros = ((timestamp - seconds) * 1e6).round_ties_even();
        if micros >= 1e6 {
            seconds += 1.0;
        }
        let seconds = seconds as i64;
        let days = seconds.div_euclid(86_400);
        let of_day = seconds.rem_euclid(86_400);
        let (year, month, day) = civil_from_days(days);
        Ok(LocalDateTime {
            year,
            month,
            day,
            hour: (of_day / 3600) as u32,
            minute: (of_day % 3600 / 60) as u32,
        })
    }
}

/// Howard Hinnant's `civil_from_days`.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((if m <= 2 { y + 1 } else { y }) as i32, m, d)
}

/// Collects mismatches and fails with the first few.
pub struct Mismatches {
    what: &'static str,
    total: usize,
    failures: Vec<String>,
}

impl Mismatches {
    pub fn new(what: &'static str) -> Self {
        Mismatches {
            what,
            total: 0,
            failures: Vec::new(),
        }
    }

    pub fn check(&mut self, ok: bool, describe: impl FnOnce() -> String) {
        self.total += 1;
        if !ok {
            self.failures.push(describe());
        }
    }

    pub fn finish(self) {
        assert!(self.total > 0, "{}: no cases", self.what);
        if !self.failures.is_empty() {
            let shown: Vec<&String> = self.failures.iter().take(8).collect();
            panic!(
                "{}: {} of {} cases differ from Python. First:\n{}",
                self.what,
                self.failures.len(),
                self.total,
                shown
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("\n---\n")
            );
        }
        eprintln!("{}: {} cases match Python", self.what, self.total);
    }
}
