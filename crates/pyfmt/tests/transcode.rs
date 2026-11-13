//! `transcode` against CPython and against the parse+encode path it
//! replaces (`loads_lenient` then `dumps`/`tornado_json_encode`).
//!
//! `data/transcode_goldens.json` holds CPython 3.12's reading of random and
//! hand-picked texts, valid and not (`data/generate_transcode_goldens.py`).
//! The property tests generate documents here, write them with random
//! whitespace, escapes and number spellings, and mutate some of them.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use pyfmt::{
    DumpOpts, JsonStyle, Members, dumps, loads_lenient, repr_f64, tornado_json_encode, transcode,
    transcode_members,
};
use serde_json::{Map, Number, Value};

const STYLES: [JsonStyle; 3] = [JsonStyle::DUMPS, JsonStyle::UNICODE, JsonStyle::TORNADO];

fn run(text: &str, style: JsonStyle) -> Option<String> {
    let mut out = String::new();
    transcode(text, style, &mut out).ok().map(|()| out)
}

/// What the parse+encode path writes.
fn reference(text: &str, style: JsonStyle) -> Option<String> {
    let value = loads_lenient(text).ok()?;
    Some(match style {
        JsonStyle::TORNADO => tornado_json_encode(&value),
        JsonStyle::UNICODE => dumps(&value, &DumpOpts::unicode()),
        _ => dumps(&value, &DumpOpts::default()),
    })
}

/// Why the parse+encode path cannot write what CPython writes for `text`,
/// if it is one of the known reasons: what `serde_json::Value` cannot hold.
fn known_difference(text: &str) -> Option<&'static str> {
    let bytes = text.as_bytes();
    if text.contains("NaN") || text.contains("Infinity") {
        return Some("NaN/Infinity tokens (null before)");
    }
    let mut run = 0;
    for &b in bytes {
        run = if b.is_ascii_digit() { run + 1 } else { 0 };
        if run >= 19 {
            return Some("integers past 64 bits (a float before)");
        }
    }
    for i in 0..bytes.len() {
        if bytes[i..].starts_with(b"-0")
            && !matches!(bytes.get(i + 2), Some(b'.' | b'e' | b'E' | b'0'..=b'9'))
        {
            return Some("-0 is the int 0 (-0.0 before)");
        }
    }
    let lower = text.to_ascii_lowercase();
    if [
        "\\ud8", "\\ud9", "\\uda", "\\udb", "\\udc", "\\udd", "\\ude", "\\udf",
    ]
    .iter()
    .any(|s| lower.contains(s))
    {
        return Some("lone surrogates (refused before)");
    }
    for (i, window) in lower.as_bytes().windows(4).enumerate() {
        if window[0] == b'e' {
            let digits = lower.as_bytes()[i + 1..]
                .iter()
                .skip_while(|b| matches!(b, b'+' | b'-'))
                .take_while(|b| b.is_ascii_digit())
                .count();
            if digits >= 3 {
                return Some("floats past the double range (refused before)");
            }
        }
    }
    None
}

#[test]
fn transcode_matches_cpython() {
    let goldens: Value =
        serde_json::from_str(include_str!("data/transcode_goldens.json")).expect("goldens");
    let cases = goldens["cases"].as_array().expect("cases");
    assert!(cases.len() > 2000);
    let mut differences: BTreeMap<&str, usize> = BTreeMap::new();
    for case in cases {
        let text = case["text"].as_str().expect("text");
        let written = |style| run(text, style);
        if case.get("error").is_some() {
            for style in STYLES {
                assert_eq!(written(style), None, "CPython refuses {text:?}");
            }
            assert!(
                transcode_members(text, ["data"], JsonStyle::DUMPS).is_err(),
                "{text:?}"
            );
            if reference(text, JsonStyle::DUMPS).is_some() {
                panic!("the parse+encode path reads {text:?}, which CPython refuses");
            }
            continue;
        }
        let default = case["default"].as_str().expect("default");
        let expected = |key: &str| match case.get(key) {
            None => Some(default.to_string()),
            Some(Value::Null) => None,
            Some(other) => Some(other.as_str().expect("string").to_string()),
        };
        assert_eq!(
            written(JsonStyle::DUMPS).as_deref(),
            Some(default),
            "{text:?}"
        );
        assert_eq!(written(JsonStyle::UNICODE), expected("unicode"), "{text:?}");
        assert_eq!(written(JsonStyle::TORNADO), expected("tornado"), "{text:?}");
        let data = case["data"].as_str().expect("data");
        match transcode_members(text, ["data"], JsonStyle::DUMPS) {
            Ok(Members::Object([found])) => {
                assert_eq!(found.as_deref().unwrap_or("null"), data, "{text:?}")
            }
            Ok(Members::NotObject(name)) => assert_eq!(format!("!{name}"), data, "{text:?}"),
            Err(error) => panic!("{text:?}: {error}"),
        }
        // Where the old path writes something else, the reason is a known one.
        if reference(text, JsonStyle::DUMPS).as_deref() != Some(default) {
            let reason = known_difference(text)
                .unwrap_or_else(|| panic!("unexplained difference for {text:?}"));
            *differences.entry(reason).or_default() += 1;
        }
    }
    eprintln!("CPython parity the parse+encode path lacked: {differences:#?}");
    assert!(differences.contains_key("NaN/Infinity tokens (null before)"));
}

/// SplitMix64: deterministic and dependency free.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

const CHARS: &[char] = &[
    'a',
    'b',
    'z',
    ' ',
    'A',
    '0',
    '9',
    '"',
    '\\',
    '/',
    '<',
    '>',
    '\n',
    '\t',
    '\r',
    '\u{8}',
    '\u{c}',
    '\0',
    '\u{1f}',
    '\u{7f}',
    '\u{e9}',
    '\u{2028}',
    '\u{2615}',
    '\u{feff}',
    '\u{ffff}',
    '\u{1f600}',
    '\u{1d11e}',
];

fn random_string(rng: &mut Rng) -> String {
    let mut s = String::new();
    for _ in 0..rng.below(8) {
        s.push(*rng.pick(CHARS));
        if rng.chance(10) {
            s.push_str("</");
        }
    }
    s
}

fn random_float(rng: &mut Rng) -> f64 {
    loop {
        let v = match rng.below(4) {
            0 => (rng.below(60000) as f64 - 30000.0) / 100.0,
            1 => f64::from_bits(rng.next()),
            2 => *rng.pick(&[
                0.0,
                -0.0,
                0.1,
                1e16,
                1e15,
                1e-5,
                1e-4,
                5e-324,
                f64::MAX,
                0.1 + 0.2,
            ]),
            _ => {
                (rng.next() as f64 / u64::MAX as f64 - 0.5) * 10f64.powi(rng.below(40) as i32 - 12)
            }
        };
        if v.is_finite() {
            return v;
        }
    }
}

fn random_value(rng: &mut Rng, depth: usize) -> Value {
    match rng.below(100) {
        n if n < 18 && depth < 5 => {
            let mut map = Map::new();
            for _ in 0..rng.below(6) {
                let key = if rng.chance(50) {
                    rng.pick(&["data", "shot", "a", "</x>", "\u{e9}"])
                        .to_string()
                } else {
                    random_string(rng)
                };
                map.insert(key, random_value(rng, depth + 1));
            }
            Value::Object(map)
        }
        n if n < 32 && depth < 5 => Value::Array(
            (0..rng.below(6))
                .map(|_| random_value(rng, depth + 1))
                .collect(),
        ),
        n if n < 60 => Value::Number(Number::from_f64(random_float(rng)).expect("finite")),
        n if n < 70 => Value::from(rng.next() as i64 >> rng.below(64)),
        n if n < 75 => Value::from(rng.next()),
        n if n < 90 => Value::String(random_string(rng)),
        n if n < 94 => Value::Bool(rng.chance(50)),
        _ => Value::Null,
    }
}

fn whitespace(rng: &mut Rng, text: &mut String) {
    if rng.chance(30) {
        for _ in 0..=rng.below(3) {
            text.push(*rng.pick(&[' ', '\t', '\n', '\r']));
        }
    }
}

fn unicode_escape(rng: &mut Rng, text: &mut String, unit: u32) {
    let escape = format!("\\u{unit:04x}");
    if rng.chance(30) {
        text.push_str(&escape.to_uppercase().replace("\\U", "\\u"));
    } else {
        text.push_str(&escape);
    }
}

/// A string as some JSON writer could spell it.
fn write_string(rng: &mut Rng, text: &mut String, s: &str) {
    text.push('"');
    for c in s.chars() {
        let short = match c {
            '"' => Some("\\\""),
            '\\' => Some("\\\\"),
            '\n' => Some("\\n"),
            '\r' => Some("\\r"),
            '\t' => Some("\\t"),
            '\u{8}' => Some("\\b"),
            '\u{c}' => Some("\\f"),
            '/' if rng.chance(30) => Some("\\/"),
            _ => None,
        };
        match short {
            Some(escape) if rng.chance(80) => text.push_str(escape),
            _ if c < ' ' || c == '"' || c == '\\' || rng.chance(15) => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    unicode_escape(rng, text, u32::from(*unit));
                }
            }
            _ => text.push(c),
        }
    }
    text.push('"');
}

/// A float as some JSON writer could spell it: repr, more digits, an
/// exponent, trailing zeros.
fn write_float(rng: &mut Rng, text: &mut String, v: f64) {
    let repr = repr_f64(v);
    match rng.below(10) {
        0..=5 => text.push_str(&repr),
        6 => text.push_str(&format!("{v:.17e}")),
        7 => {
            let exponent = format!("{v:e}");
            text.push_str(&exponent.replace('e', if rng.chance(50) { "E" } else { "e" }));
        }
        8 if !repr.contains('e') => {
            text.push_str(&repr);
            text.push_str(&"0".repeat(1 + rng.below(3) as usize));
        }
        _ => text.push_str(&format!("{v:?}")),
    }
}

fn write_value(rng: &mut Rng, text: &mut String, value: &Value) {
    match value {
        Value::Number(n) if n.is_f64() => write_float(rng, text, n.as_f64().expect("f64")),
        Value::String(s) => write_string(rng, text, s),
        Value::Array(items) => {
            text.push('[');
            whitespace(rng, text);
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    text.push(',');
                    whitespace(rng, text);
                }
                write_value(rng, text, item);
                whitespace(rng, text);
            }
            text.push(']');
        }
        Value::Object(map) => {
            // Sometimes the first key again, with another value.
            let repeated = random_value(rng, 4);
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            if let Some((key, _)) = entries.first().copied().filter(|_| rng.chance(20)) {
                entries.push((key, &repeated));
            }
            text.push('{');
            whitespace(rng, text);
            for (i, (key, item)) in entries.into_iter().enumerate() {
                if i > 0 {
                    text.push(',');
                    whitespace(rng, text);
                }
                write_string(rng, text, key);
                whitespace(rng, text);
                text.push(':');
                whitespace(rng, text);
                write_value(rng, text, item);
                whitespace(rng, text);
            }
            text.push('}');
        }
        other => text.push_str(&other.to_string()),
    }
}

fn random_text(rng: &mut Rng) -> String {
    let value = random_value(rng, 0);
    let mut text = String::new();
    whitespace(rng, &mut text);
    write_value(rng, &mut text, &value);
    whitespace(rng, &mut text);
    text
}

/// Valid documents: the same bytes as the parse+encode path, in every style.
#[test]
fn transcode_matches_parse_and_encode() {
    let mut rng = Rng(1);
    for _ in 0..20_000 {
        let text = random_text(&mut rng);
        for style in STYLES {
            let expected = reference(&text, style).expect("generated documents are valid");
            assert_eq!(
                run(&text, style).as_deref(),
                Some(expected.as_str()),
                "{text:?}"
            );
        }
    }
}

/// Broken documents: refused where the parse+encode path refuses them, and
/// written the same where both read them, apart from the known differences.
#[test]
fn mutated_documents_match_parse_and_encode() {
    const BYTES: &[u8] = b"\"{}[],:\\ 0123456789-+.eEtfnNIau/";
    let mut rng = Rng(2);
    let mut refused = 0;
    for _ in 0..30_000 {
        let mut text: Vec<char> = random_text(&mut rng).chars().collect();
        for _ in 0..=rng.below(2) {
            if text.is_empty() {
                break;
            }
            let at = rng.below(text.len() as u64) as usize;
            let c = char::from(*rng.pick(BYTES));
            match rng.below(10) {
                0..=3 => {
                    text.remove(at);
                }
                4..=7 => text.insert(at, c),
                8 => text[at] = c,
                _ => text.truncate(at),
            }
        }
        let text: String = text.into_iter().collect();
        let ours = run(&text, JsonStyle::TORNADO);
        let theirs = reference(&text, JsonStyle::TORNADO);
        refused += usize::from(ours.is_none());
        if ours != theirs {
            assert!(
                known_difference(&text).is_some(),
                "{text:?}: {ours:?} != {theirs:?}"
            );
        }
        // Checking alone accepts what writing accepts.
        let members = transcode_members(&text, ["data"], JsonStyle::DUMPS);
        assert_eq!(
            members.is_ok(),
            run(&text, JsonStyle::DUMPS).is_some(),
            "{text:?}"
        );
    }
    assert!(refused > 5_000, "{refused}");
}

/// The floats copied as they are: whatever the spelling, the output is
/// `repr(float(token))`.
#[test]
fn floats_are_written_as_their_repr() {
    let mut rng = Rng(3);
    for _ in 0..300_000 {
        let v = random_float(&mut rng);
        let repr = repr_f64(v);
        assert_eq!(run(&repr, JsonStyle::DUMPS).as_deref(), Some(repr.as_str()));
    }
    // Decimal spellings around the 15-digit and layout boundaries.
    for _ in 0..300_000 {
        let digits: String = (0..1 + rng.below(18))
            .map(|_| char::from(b'0' + rng.below(10) as u8))
            .collect();
        let digits = digits.trim_start_matches('0');
        let digits = if digits.is_empty() { "0" } else { digits };
        let point = rng.below(digits.len() as u64 + 6) as usize;
        let token = if point == 0 {
            format!("0.{digits}")
        } else if point < digits.len() {
            format!("{}.{}", &digits[..point], &digits[point..])
        } else if point == digits.len() {
            format!("{digits}.0")
        } else {
            format!("0.{}{digits}", "0".repeat(point - digits.len()))
        };
        let token = if rng.chance(50) {
            format!("-{token}")
        } else {
            token
        };
        let expected = repr_f64(token.parse::<f64>().expect("decimal"));
        assert_eq!(
            run(&token, JsonStyle::DUMPS).as_deref(),
            Some(expected.as_str()),
            "{token}"
        );
    }
}
