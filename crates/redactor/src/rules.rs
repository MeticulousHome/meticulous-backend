//! The shared redaction rules: a port of `log_redactor/redactor.py`.
//!
//! `log_redactor` is a submodule shared with meticulous-watcher, and both
//! services load the same per-device key, so a value must get the same token
//! here, in the Python backend and in the watcher (REDACTION_SPEC.md, I7).
//! Every rule therefore reproduces CPython `re` semantics exactly, including
//! the backtracking behind `\b` and the lookarounds the `regex` crate lacks
//! (spec section 8.6), CPython's `\w`/`\s`/`\d` classes and its
//! `re.IGNORECASE` folds. Rule order is load-bearing (spec section 6).

use std::borrow::Cow;
use std::cmp::Reverse;

use hmac::{Hmac, Mac};
use regex::{Captures, Regex};
use sha2::Sha256;

use crate::filter::{KeySource, LogRedactor};
use crate::pychar::{
    SPACE_CLASS, char_len_at, ignorecase_eq, ignorecase_pattern, is_decimal, is_space, is_word,
    is_word_or_dash, next_char, prev_char, py_len, py_repr, py_strip, split_lines_keepends,
    strip_quotes,
};

/// Replacement for credentials: unconditional, no correlation.
pub const REDACTED: &str = "[REDACTED]";
/// Where both services keep the per-device key (`REDACTION_KEY_PATH` overrides
/// it in the backend).
pub const DEFAULT_KEY_PATH: &str = "/root/.redaction_key";
/// Length of a valid key file.
pub const KEY_SIZE: usize = 32;
/// Learned or seeded values shorter than this (in code points) are not swept.
pub const MIN_SWEEP_LEN: usize = 4;
/// What a timezone location is coarsened to.
pub const TZ_MASK: &str = "*****";

/// `_CANCEL_CHECK_INTERVAL`: pass 1 polls the cancellation predicate on every
/// 500th line.
const CANCEL_CHECK_INTERVAL: usize = 500;

/// `pseudonym(kind, value, key)`: `[KIND_xxxxxxxx]`, the first 8 hex digits of
/// HMAC-SHA256(key, kind || 0x00 || value).
pub fn pseudonym(kind: &str, value: &str, key: &[u8]) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key).expect("HMAC-SHA256 accepts keys of any length");
    mac.update(kind.as_bytes());
    mac.update(&[0]);
    mac.update(value.as_bytes());
    let digest = mac.finalize().into_bytes();
    format!(
        "[{kind}_{:02x}{:02x}{:02x}{:02x}]",
        digest[0], digest[1], digest[2], digest[3]
    )
}

/// `RE_ALREADY_DONE.match(s)`: `s` is `[REDACTED]` or a pseudonym token.
///
/// PARITY: the Python pattern ends in `$`, which also matches before one
/// trailing `\n`, so `"[REDACTED]\n"` counts as already redacted.
pub fn is_already_redacted(s: &str) -> bool {
    let s = s.strip_suffix('\n').unwrap_or(s);
    if s == REDACTED {
        return true;
    }
    let Some(inner) = s.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
        return false;
    };
    let Some((kind, hex)) = inner.split_once('_') else {
        return false;
    };
    matches!(kind, "SSID" | "MAC" | "IPV6")
        && hex.len() == 8
        && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Why a key file cannot be used (`_read_key` raises `ValueError`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("redaction key at {} must be exactly {KEY_SIZE} bytes", py_repr(path))]
pub struct KeyError {
    /// The path the key was read from.
    pub path: String,
    /// How many bytes it held.
    pub len: usize,
}

impl KeyError {
    /// The Python exception class, for the log filter's failure placeholder.
    pub fn python_exception(&self) -> &'static str {
        "ValueError"
    }
}

/// `_read_key`'s check on the bytes of an existing key file.
///
/// Reading, creating (`O_EXCL`, mode 0600, `fsync`) and chmodding the file is
/// IO and lives in the backend; see `load_key` in `redactor.py`.
pub fn check_key(path: &str, bytes: &[u8]) -> Result<(), KeyError> {
    if bytes.len() == KEY_SIZE {
        Ok(())
    } else {
        Err(KeyError {
            path: path.to_owned(),
            len: bytes.len(),
        })
    }
}

/// `RedactionState`: the KnownWifis block tracking that survives across calls
/// for callers that feed one log record at a time. Learned SSIDs deliberately
/// do not live here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RedactionState {
    /// Indent (in code points) of the `KnownWifis:` line of the open block.
    pub known_wifis_indent: Option<usize>,
    /// Indent of the keys that are SSIDs, fixed by the first deeper key.
    pub ssid_key_indent: Option<usize>,
}

/// `redact()`'s result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redacted {
    /// The redacted text. Line count is preserved except where Python itself
    /// drops a line break (see the R4i note on [`Redactor::redact`]).
    pub text: String,
    /// The SSIDs learned in this call, each once, in the order first learned
    /// (Python returns a `set`).
    pub learned: Vec<String>,
}

/// `RedactionCancelled`: the predicate fired; no partial output is returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("redaction cancelled")]
pub struct RedactionCancelled;

/// The compiled rules, independent of the key. Cloning shares the compiled
/// regexes.
#[derive(Clone)]
pub(crate) struct Rules {
    cred_kv: Regex,
    cred_nm: Regex,
    mac: Regex,
    ssid: Vec<SsidRule>,
    tz_abbrev: Regex,
}

#[derive(Clone)]
struct SsidRule {
    re: Regex,
    /// The Python pattern starts with `\b` before a word character.
    word_boundary: bool,
    /// A literal every match contains: lines without it skip the regex.
    literal: Option<&'static str>,
}

/// `TIMEZONE` areas, in the pattern's order.
const TZ_AREAS: [&str; 16] = [
    "Africa",
    "America",
    "Antarctica",
    "Arctic",
    "Asia",
    "Atlantic",
    "Australia",
    "Brazil",
    "Canada",
    "Chile",
    "Etc",
    "Europe",
    "Indian",
    "Mexico",
    "Pacific",
    "US",
];

/// `TIMEZONE_ALLOWLIST`: zero-offset zones identify nobody.
const TZ_ALLOWLIST: [&str; 5] = [
    "Etc/UTC",
    "Etc/GMT",
    "Etc/Universal",
    "Etc/Zulu",
    "Etc/Greenwich",
];

fn compile(pattern: &str) -> Regex {
    Regex::new(pattern).expect("redaction patterns are constants and compile")
}

impl Rules {
    pub(crate) fn new() -> Rules {
        let ci = ignorecase_pattern;
        let ws = SPACE_CLASS;
        // _CRED_KEY under re.IGNORECASE.
        let cred_key = [
            format!("{}_?{}", ci("root"), ci("password")),
            format!("{}_?{}", ci("ap"), ci("password")),
            ci("passwd"),
            ci("password"),
            ci("passphrase"),
            ci("psk"),
            format!("{}_?{}_?{}", ci("pre"), ci("shared"), ci("key")),
            ci("secret"),
            ci("token"),
            format!("{}[_-]?{}", ci("api"), ci("key")),
            ci("authorization"),
            ci("bearer"),
        ]
        .join("|");
        // _CRED_VALUE under re.IGNORECASE: the token alternatives fold too.
        let cred_value = format!(
            r#"(?P<val>\[{redacted}\]|\[(?:{ssid}|{mac}|{ipv6})_[0-9a-fA-F]{{8}}\]|'[^'\n]*'|"[^"\n]*"|[^{ws},;}}\]]+)"#,
            redacted = ci("REDACTED"),
            ssid = ci("SSID"),
            mac = ci("MAC"),
            ipv6 = ci("IPV6"),
        );
        // RE_CRED_KV without its `\b`s: the leading one is checked by hand
        // (see sub_cred_kv) and the trailing one always holds, because what
        // may follow the key is a quote, whitespace, `:` or `=`.
        let cred_kv = compile(&format!(
            r#"(?P<key>["']?(?:{cred_key})["']?)(?P<sep>[{ws}]*[:=][{ws}]*){cred_value}"#
        ));
        let cred_nm = compile(&format!(
            r"(?P<key>{added}[{ws}]+'(?:{cred_key})'[{ws}]+{value}[{ws}]+)(?P<val>'[^'\n]*')",
            added = ci("added"),
            value = ci("value"),
        ));
        // RE_MAC without its `\b`s, which are checked by hand.
        let mac = compile(r"(?:[0-9A-Fa-f]{2}:){5}[0-9A-Fa-f]{2}");
        let ssid_ci = ci("ssid");
        // (pattern, starts with `\b`, a literal every match contains)
        let ssid_patterns: [(String, bool, Option<&'static str>); 9] = [
            (
                r"(?P<pre>Config: added 'ssid' value ')(?P<val>[^'\n]{1,32})(?P<post>')".into(),
                false,
                Some("Config: added 'ssid' value '"),
            ),
            (
                r"(?P<pre>SSID=')(?P<val>[^'\n]{1,32})(?P<post>')".into(),
                false,
                Some("SSID='"),
            ),
            (
                r"(?P<pre>associate with SSID ')(?P<val>[^'\n]{1,32})(?P<post>')".into(),
                false,
                Some("associate with SSID '"),
            ),
            (
                r#"(?P<pre>wireless network ")(?P<val>[^"\n]{1,32})(?P<post>")"#.into(),
                false,
                Some("wireless network \""),
            ),
            (
                r"(?P<pre>access point ')(?P<val>[^'\n]{1,32})(?P<post>')".into(),
                true,
                Some("access point '"),
            ),
            (
                r"(?P<pre>connection ')(?P<val>[^'\n]{1,32})(?P<post>')".into(),
                true,
                Some("connection '"),
            ),
            (
                r"(?P<pre>policy: set ')(?P<val>[^'\n]{1,32})(?P<post>')".into(),
                false,
                Some("policy: set '"),
            ),
            (
                format!(
                    r#"(?P<pre>{ssid_ci}[{ws}]*[:=][{ws}]*["'])(?P<val>[^"'\n]{{1,32}})(?P<post>["'])"#
                ),
                true,
                None,
            ),
            (
                format!(
                    r#"(?P<pre>{ssid_ci}[{ws}]*[:=][{ws}]*)(?P<val>[^{ws}"'#,;}}\]][^\n#,;}}\]]{{0,31}})(?P<post>)"#
                ),
                true,
                None,
            ),
        ];
        let ssid = ssid_patterns
            .into_iter()
            .map(|(pattern, word_boundary, literal)| SsidRule {
                re: compile(&pattern),
                word_boundary,
                literal,
            })
            .collect();
        let tz_abbrev = compile(&format!(
            r#"(?P<pre>/\*\*\*\*\*['"]?[{ws}]*)\([^)\n]{{0,32}}\)"#
        ));
        Rules {
            cred_kv,
            cred_nm,
            mac,
            ssid,
            tz_abbrev,
        }
    }

    /// `redact(text, key, state, cancelled)`.
    pub(crate) fn redact(
        &self,
        text: &str,
        key: &[u8],
        state: &mut RedactionState,
        mut cancelled: Option<&mut dyn FnMut() -> bool>,
    ) -> Result<Redacted, RedactionCancelled> {
        let mut learned: Vec<String> = Vec::new();
        let mut out = String::with_capacity(text.len());

        // ---------------- pass 1 ----------------
        for (index, line) in split_lines_keepends(text).enumerate() {
            if index % CANCEL_CHECK_INTERVAL == 0 {
                check_cancelled(&mut cancelled)?;
            }
            self.redact_line(line, key, state, &mut learned, &mut out);
        }

        // ---------------- pass 2: literal sweep ----------------
        // Longest first. PARITY: Python sorts a `set`, so the order of two
        // learned SSIDs of equal length is hash-randomised; it only matters
        // when their occurrences overlap. Here ties keep the learn order.
        let mut order: Vec<&String> = learned.iter().collect();
        order.sort_by_key(|ssid| Reverse(py_len(ssid)));
        let mut text = out;
        for ssid in order {
            check_cancelled(&mut cancelled)?;
            if py_len(ssid) < MIN_SWEEP_LEN || is_already_redacted(ssid) {
                continue;
            }
            let token = pseudonym("SSID", ssid, key);
            if let Cow::Owned(swept) = sweep_literal(&text, ssid, &token) {
                text = swept;
            }
        }

        Ok(Redacted { text, learned })
    }

    fn redact_line(
        &self,
        line: &str,
        key: &[u8],
        state: &mut RedactionState,
        learned: &mut Vec<String>,
        out: &mut String,
    ) {
        let mut cur: Cow<'_, str> = Cow::Borrowed(line);
        fn apply<'a>(cur: &mut Cow<'a, str>, f: impl FnOnce(&str) -> Cow<'_, str>) {
            let next = match f(cur.as_ref()) {
                Cow::Owned(s) => Some(s),
                Cow::Borrowed(_) => None,
            };
            if let Some(s) = next {
                *cur = Cow::Owned(s);
            }
        }

        // The guards below are conditions every match of the rule satisfies,
        // checked on the current text, so skipping a rule never changes output.
        if cur.contains([':', '=']) {
            apply(&mut cur, |l| self.sub_cred_kv(l));
        }
        if count_byte(&cur, b'\'') >= 4 {
            apply(&mut cur, |l| self.sub_cred_nm(l));
        }
        if count_byte(&cur, b':') >= 5 {
            apply(&mut cur, |l| self.sub_mac(l, key));
        }
        if cur.contains("::") || count_byte(&cur, b':') >= 7 {
            apply(&mut cur, |l| sub_ipv6(l, key));
        }
        for rule in &self.ssid {
            let possible = match rule.literal {
                Some(literal) => cur.contains(literal),
                None => contains_ignorecase_ssid(&cur),
            };
            if possible {
                apply(&mut cur, |l| sub_ssid(rule, l, key, learned));
            }
        }
        if cur.contains('/') {
            apply(&mut cur, sub_timezone);
        }
        if cur.contains("/*****") {
            apply(&mut cur, |l| {
                sub_regex(&self.tz_abbrev, l, |_, caps| {
                    Sub::Replace(format!("{}({TZ_MASK})", &caps["pre"]))
                })
            });
        }

        // KnownWifis block: keys are SSIDs.
        let line: &str = &cur;
        let (body, ending) = split_line_ending(line);
        if let Some(conf) = body.find("CONF:") {
            let (pre, yaml) = body.split_at(conf + "CONF:".len());
            let mut rewritten = None;
            if let Some(block_indent) = state.known_wifis_indent {
                match match_yaml_key(yaml) {
                    Some(km) if km.indent_len > block_indent => {
                        // First deeper key fixes the SSID level, even when it is
                        // already redacted (spec section 8.4).
                        let ssid_indent = *state.ssid_key_indent.get_or_insert(km.indent_len);
                        let val = py_strip(km.key);
                        if km.indent_len == ssid_indent && !is_already_redacted(val) {
                            learn(learned, val);
                            rewritten = Some(format!(
                                "{pre}{}{}{}{ending}",
                                km.indent,
                                pseudonym("SSID", val, key),
                                km.rest
                            ));
                        }
                    }
                    _ => {
                        if !py_strip(yaml).is_empty() {
                            state.known_wifis_indent = None;
                            state.ssid_key_indent = None;
                        }
                    }
                }
            }
            if let Some((indent_len, inline)) = match_known_wifis(yaml)
                && matches!(inline, "" | "|" | ">")
            {
                state.known_wifis_indent = Some(indent_len);
                state.ssid_key_indent = None;
            }
            if let Some(rewritten) = rewritten {
                out.push_str(&rewritten);
                return;
            }
        }
        out.push_str(line);
    }

    /// Rule 1a, `RE_CRED_KV.sub(_sub_cred, line)`.
    fn sub_cred_kv<'t>(&self, line: &'t str) -> Cow<'t, str> {
        sub_regex(&self.cred_kv, line, |text, caps| {
            let whole = caps.get(0).expect("group 0 always participates");
            let key = &caps["key"];
            // `\b` before the key word. With the opening quote taken it sits
            // between a quote and a letter and always holds.
            if !key.starts_with(['"', '\'']) && prev_char(text, whole.start()).is_some_and(is_word)
            {
                return Sub::Veto;
            }
            sub_cred(key, &caps["sep"], &caps["val"])
        })
    }

    /// Rule 1b, `RE_CRED_NM.sub(_sub_cred, line)`.
    fn sub_cred_nm<'t>(&self, line: &'t str) -> Cow<'t, str> {
        sub_regex(&self.cred_nm, line, |_, caps| {
            sub_cred(&caps["key"], "", &caps["val"])
        })
    }

    /// Rule 2, MAC addresses.
    fn sub_mac<'t>(&self, line: &'t str, key: &[u8]) -> Cow<'t, str> {
        sub_regex(&self.mac, line, |text, caps| {
            let m = caps.get(0).expect("group 0 always participates");
            if prev_char(text, m.start()).is_some_and(is_word)
                || next_char(text, m.end()).is_some_and(is_word)
            {
                return Sub::Veto;
            }
            let lower = m.as_str().to_ascii_lowercase();
            if mac_allowlisted(&lower) {
                Sub::Keep
            } else {
                Sub::Replace(pseudonym("MAC", &lower, key))
            }
        })
    }
}

fn count_byte(s: &str, byte: u8) -> usize {
    s.bytes().filter(|&b| b == byte).count()
}

/// Whether `s` contains `ssid` as `re.IGNORECASE` reads it (R4h and R4i
/// both start with it). Only the last letter has no non-ASCII fold, so
/// look for `d`/`D` and check the three characters before it.
fn contains_ignorecase_ssid(s: &str) -> bool {
    s.match_indices(['d', 'D']).any(|(i, _)| {
        let mut before = s[..i].chars().rev();
        ['i', 's', 's']
            .iter()
            .all(|&letter| before.next().is_some_and(|c| ignorecase_eq(c, letter)))
    })
}

fn check_cancelled(
    cancelled: &mut Option<&mut dyn FnMut() -> bool>,
) -> Result<(), RedactionCancelled> {
    if let Some(predicate) = cancelled.as_mut()
        && predicate()
    {
        return Err(RedactionCancelled);
    }
    Ok(())
}

fn learn(learned: &mut Vec<String>, value: &str) {
    if !learned.iter().any(|known| known == value) {
        learned.push(value.to_owned());
    }
}

/// `_sub_cred`.
fn sub_cred(key: &str, sep: &str, val: &str) -> Sub {
    if is_already_redacted(strip_quotes(val)) {
        return Sub::Keep;
    }
    // PARITY: an unclosed quote is matched by the bare-value branch, and the
    // replacement still wraps [REDACTED] in that quote on both sides.
    let quote = match val.chars().next() {
        Some(q @ ('\'' | '"')) => q.to_string(),
        _ => String::new(),
    };
    Sub::Replace(format!("{key}{sep}{quote}{REDACTED}{quote}"))
}

/// `MAC_ALLOWLIST` on an already lowercased MAC.
fn mac_allowlisted(lower: &str) -> bool {
    lower == "ff:ff:ff:ff:ff:ff"
        || lower == "00:00:00:00:00:00"
        || lower.starts_with("33:33:")
        || lower.starts_with("01:00:5e:")
}

/// `CONNECTION_ALLOWLIST.match(val)`:
/// `^(?:lo|usb\d+|eth\d+|Wired connection \d+)$`, case-insensitive.
fn connection_allowlisted(val: &str) -> bool {
    let val = val.strip_suffix('\n').unwrap_or(val);
    let starts_ci = |s: &str, word: &str| -> Option<usize> {
        let mut chars = s.char_indices();
        for w in word.chars() {
            let (_, c) = chars.next()?;
            let ok = if w.is_ascii_alphabetic() {
                ignorecase_eq(c, w)
            } else {
                c == w
            };
            if !ok {
                return None;
            }
        }
        Some(chars.next().map_or(s.len(), |(i, _)| i))
    };
    if starts_ci(val, "lo") == Some(val.len()) {
        return true;
    }
    ["usb", "eth", "wired connection "].iter().any(|word| {
        starts_ci(val, word).is_some_and(|end| {
            let digits = &val[end..];
            !digits.is_empty() && digits.chars().all(is_decimal)
        })
    })
}

/// What a `re.sub` callback decides for one match.
pub(crate) enum Sub {
    /// The match does not exist for Python (a `\b` or lookaround the `regex`
    /// crate cannot express failed): rescan from the next character.
    Veto,
    /// The callback returned `m.group(0)`: the text stays, the scan moves on.
    Keep,
    /// Replace the match.
    Replace(String),
}

/// `pattern.sub(callback, text)` for a `regex` crate pattern.
pub(crate) fn sub_regex<'t>(
    re: &Regex,
    text: &'t str,
    mut decide: impl FnMut(&'t str, &Captures<'t>) -> Sub,
) -> Cow<'t, str> {
    let mut out: Option<String> = None;
    let mut copied = 0;
    let mut pos = 0;
    while pos <= text.len() {
        // `find_at` does not allocate; captures are only built for a match,
        // and from its start they are that same leftmost-first match.
        let Some(found) = re.find_at(text, pos) else {
            break;
        };
        let Some(caps) = re.captures_at(text, found.start()) else {
            break;
        };
        let m = caps.get(0).expect("group 0 always participates");
        match decide(text, &caps) {
            Sub::Veto => {
                pos = m.start() + char_len_at(text, m.start());
                continue;
            }
            Sub::Keep => {}
            Sub::Replace(replacement) => {
                let o = out.get_or_insert_with(|| String::with_capacity(text.len() + 16));
                o.push_str(&text[copied..m.start()]);
                o.push_str(&replacement);
                copied = m.end();
            }
        }
        // None of the patterns can match empty; step anyway so a future one
        // cannot loop forever.
        pos = if m.end() > m.start() {
            m.end()
        } else {
            m.end() + char_len_at(text, m.end())
        };
    }
    finish(text, out, copied)
}

fn finish(text: &str, out: Option<String>, copied: usize) -> Cow<'_, str> {
    match out {
        None => Cow::Borrowed(text),
        Some(mut o) => {
            o.push_str(&text[copied..]);
            Cow::Owned(o)
        }
    }
}

/// Whether the byte before `i` is an ASCII word character: then no match that
/// needs a non-word character before it (`\b` before a word character, or
/// `(?<![:\w])`) can start at `i`.
fn after_ascii_word(bytes: &[u8], i: usize) -> bool {
    i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_')
}

/// `pattern.sub(callback, text)` for a hand-written matcher: `try_at(p)`
/// returns the end of the match at `p` and its replacement (`None` keeps the
/// text). Only positions `candidate` accepts are tried; it must accept every
/// position where a match can start, and only at ASCII bytes, which are
/// character boundaries.
fn sub_scan<'t>(
    text: &'t str,
    candidate: impl Fn(&[u8], usize) -> bool,
    mut try_at: impl FnMut(usize) -> Option<(usize, Option<String>)>,
) -> Cow<'t, str> {
    let bytes = text.as_bytes();
    let mut out: Option<String> = None;
    let mut copied = 0;
    let mut pos = 0;
    while pos < bytes.len() {
        if !candidate(bytes, pos) {
            pos += 1;
            continue;
        }
        match try_at(pos) {
            None => pos += 1,
            Some((end, replacement)) => {
                if let Some(replacement) = replacement {
                    let o = out.get_or_insert_with(|| String::with_capacity(text.len() + 16));
                    o.push_str(&text[copied..pos]);
                    o.push_str(&replacement);
                    copied = end;
                }
                pos = end.max(pos + 1);
            }
        }
    }
    finish(text, out, copied)
}

fn hex_run(bytes: &[u8], i: usize) -> usize {
    bytes.get(i..).map_or(0, |rest| {
        rest.iter().take_while(|b| b.is_ascii_hexdigit()).count()
    })
}

/// Rule 3, `RE_IPV6.sub(...)`.
fn sub_ipv6<'t>(line: &'t str, key: &[u8]) -> Cow<'t, str> {
    sub_scan(
        line,
        // Every branch needs a non-word character before it.
        |b, i| (b[i].is_ascii_hexdigit() || b[i] == b':') && !after_ascii_word(b, i),
        |p| {
            let end = match_ipv6_at(line, p)?;
            let matched = &line[p..end];
            let replacement = if matched == "::1" || matched == "::" {
                None
            } else {
                Some(pseudonym("IPV6", &matched.to_ascii_lowercase(), key))
            };
            Some((end, replacement))
        },
    )
}

/// The end of the `RE_IPV6` match starting at `p`, if any.
///
/// Python's pattern, with H = `[0-9A-Fa-f]{1,4}`:
///
/// ```text
/// A  \b(?:H:){7}H\b
/// B  (?<![:\w])::(?:H(?::H){0,6})?(?![:\w])
/// C  (?<![:\w])(?:H:){1,7}:(?:H(?::H){0,6})?(?![:\w])
/// ```
///
/// Hex digits, `:` and the characters around them are disjoint, so each H
/// that must be followed by `:` takes a whole run of 1..=4 digits, and the
/// only end a trailing lookahead can accept is that of the longest chain:
/// every shorter alternative backtracking would try ends before a hex digit
/// or a `:`. The branches are tried in order, as alternation does.
fn match_ipv6_at(text: &str, p: usize) -> Option<usize> {
    let b = text.as_bytes();
    let prev = prev_char(text, p);
    let prev_word = prev.is_some_and(is_word);
    let prev_colon_or_word = prev_word || prev == Some(':');

    if !prev_word && b[p].is_ascii_hexdigit() {
        let mut i = p;
        let mut groups = 0;
        while groups < 7 {
            let r = hex_run(b, i);
            if !(1..=4).contains(&r) || b.get(i + r) != Some(&b':') {
                break;
            }
            i += r + 1;
            groups += 1;
        }
        if groups == 7 {
            let r = hex_run(b, i);
            if (1..=4).contains(&r) && !next_char(text, i + r).is_some_and(is_word) {
                return Some(i + r);
            }
        }
    }

    if prev_colon_or_word {
        return None;
    }

    if b[p] == b':' && b.get(p + 1) == Some(&b':') {
        return ipv6_tail(text, p + 2);
    }

    if b[p].is_ascii_hexdigit() {
        let mut i = p;
        let mut groups = 0;
        while groups < 7 {
            let r = hex_run(b, i);
            if !(1..=4).contains(&r) || b.get(i + r) != Some(&b':') {
                break;
            }
            i += r + 1;
            groups += 1;
        }
        if groups >= 1 && b.get(i) == Some(&b':') {
            return ipv6_tail(text, i + 1);
        }
    }
    None
}

/// `(?:H(?::H){0,6})?(?![:\w])` starting at `s`.
fn ipv6_tail(text: &str, s: usize) -> Option<usize> {
    let b = text.as_bytes();
    let blocked = |at: usize| next_char(text, at).is_some_and(|c| c == ':' || is_word(c));
    let r = hex_run(b, s);
    if r == 0 {
        return (!blocked(s)).then_some(s);
    }
    if r > 4 {
        return None;
    }
    let mut i = s + r;
    let mut groups = 1;
    while groups < 7 && b.get(i) == Some(&b':') {
        let r = hex_run(b, i + 1);
        if !(1..=4).contains(&r) {
            break;
        }
        i += 1 + r;
        groups += 1;
    }
    (!blocked(i)).then_some(i)
}

/// Rule 4, one `RE_SSID` pattern with `_sub_ssid`.
fn sub_ssid<'t>(
    rule: &SsidRule,
    line: &'t str,
    key: &[u8],
    learned: &mut Vec<String>,
) -> Cow<'t, str> {
    sub_regex(&rule.re, line, |text, caps| {
        let whole = caps.get(0).expect("group 0 always participates");
        if rule.word_boundary && prev_char(text, whole.start()).is_some_and(is_word) {
            return Sub::Veto;
        }
        // PARITY: the unquoted R4i value may run into a `\r` or another
        // line terminator at the end of the line; Python learns, hashes and
        // replaces it together with that terminator.
        let val = strip_quotes(&caps["val"]);
        if val.is_empty() || connection_allowlisted(val) || is_already_redacted(val) {
            return Sub::Keep;
        }
        learn(learned, val);
        Sub::Replace(format!(
            "{}{}{}",
            &caps["pre"],
            pseudonym("SSID", val, key),
            caps.name("post").map_or("", |m| m.as_str())
        ))
    })
}

fn is_tz_loc_char(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn tz_loc_run(b: &[u8], i: usize) -> usize {
    b.get(i..).map_or(0, |rest| {
        rest.iter().take_while(|&&c| is_tz_loc_char(c)).count()
    })
}

/// Rule 7, `RE_TIMEZONE.sub(...)`.
fn sub_timezone(line: &str) -> Cow<'_, str> {
    sub_scan(
        line,
        // The area starts with one of these letters, after a `\b`.
        |b, i| {
            matches!(b[i], b'A' | b'B' | b'C' | b'E' | b'I' | b'M' | b'P' | b'U')
                && !after_ascii_word(b, i)
        },
        |p| {
            let (area, end) = match_timezone_at(line, p)?;
            let matched = &line[p..end];
            let replacement = if TZ_ALLOWLIST.contains(&matched) {
                None
            } else {
                Some(format!("{area}/{TZ_MASK}"))
            };
            Some((end, replacement))
        },
    )
}

/// The `RE_TIMEZONE` match at `p`:
///
/// ```text
/// \b(?P<area>Africa|...|US)/(?P<loc>[A-Za-z_]+(?:[+-]\d{1,2})?(?:/[A-Za-z_]+)?)\b
/// ```
///
/// The trailing `\b` makes backtracking observable (`Etc/GMT-6x` matches as
/// `Etc/GMT`), so the quantifiers are tried in the order Python tries them.
fn match_timezone_at(text: &str, p: usize) -> Option<(&'static str, usize)> {
    if prev_char(text, p).is_some_and(is_word) {
        return None;
    }
    let rest = &text[p..];
    let area = TZ_AREAS
        .iter()
        .find(|area| rest.starts_with(*area) && rest[area.len()..].starts_with('/'))?;
    let b = text.as_bytes();
    let q = p + area.len() + 1;
    // `\b` after the location: what precedes it is always a word character.
    let boundary = |at: usize| !next_char(text, at).is_some_and(is_word);
    let second_segment = |s: usize| -> Option<usize> {
        if b.get(s) == Some(&b'/') {
            let r2 = tz_loc_run(b, s + 1);
            for l2 in (1..=r2).rev() {
                if boundary(s + 1 + l2) {
                    return Some(s + 1 + l2);
                }
            }
        }
        boundary(s).then_some(s)
    };
    let r = tz_loc_run(b, q);
    for l1 in (1..=r).rev() {
        let pos = q + l1;
        if matches!(b.get(pos), Some(b'+' | b'-')) {
            let mut digit_ends = Vec::with_capacity(2);
            let mut at = pos + 1;
            while digit_ends.len() < 2 {
                match next_char(text, at) {
                    Some(c) if is_decimal(c) => {
                        at += c.len_utf8();
                        digit_ends.push(at);
                    }
                    _ => break,
                }
            }
            for &end in digit_ends.iter().rev() {
                if let Some(end) = second_segment(end) {
                    return Some((area, end));
                }
            }
        }
        if let Some(end) = second_segment(pos) {
            return Some((area, end));
        }
    }
    None
}

/// `(?<![\w-])<literal>(?![\w-])` substituted with `replacement`.
pub(crate) fn sweep_literal<'t>(text: &'t str, literal: &str, replacement: &str) -> Cow<'t, str> {
    let mut out: Option<String> = None;
    let mut copied = 0;
    let mut pos = 0;
    while let Some(found) = text.get(pos..).and_then(|rest| rest.find(literal)) {
        let start = pos + found;
        let end = start + literal.len();
        if prev_char(text, start).is_some_and(is_word_or_dash)
            || next_char(text, end).is_some_and(is_word_or_dash)
        {
            pos = start + char_len_at(text, start);
            continue;
        }
        let o = out.get_or_insert_with(|| String::with_capacity(text.len() + 16));
        o.push_str(&text[copied..start]);
        o.push_str(replacement);
        copied = end;
        pos = end;
    }
    finish(text, out, copied)
}

/// Strip the line ending the KnownWifis code recognises: only `\r\n`, `\n`
/// and `\r`, although `splitlines` also breaks at other terminators.
fn split_line_ending(line: &str) -> (&str, &str) {
    if let Some(body) = line.strip_suffix("\r\n") {
        (body, "\r\n")
    } else if line.ends_with(['\n', '\r']) {
        line.split_at(line.len() - 1)
    } else {
        (line, "")
    }
}

struct YamlKey<'a> {
    indent: &'a str,
    indent_len: usize,
    key: &'a str,
    rest: &'a str,
}

/// `RE_YAML_KEY.match(s)`: `^(\s*)([^\s:#][^:\n]*?)(:.*)$`. `s` never holds
/// a `\n` (it is one line's body), so the key runs to the first `:`.
fn match_yaml_key(s: &str) -> Option<YamlKey<'_>> {
    let indent_end = s.find(|c: char| !is_space(c))?;
    let first = next_char(s, indent_end)?;
    if first == ':' || first == '#' {
        return None;
    }
    let after_first = indent_end + first.len_utf8();
    let colon = after_first + s[after_first..].find(':')?;
    let indent = &s[..indent_end];
    Some(YamlKey {
        indent,
        indent_len: py_len(indent),
        key: &s[indent_end..colon],
        rest: &s[colon..],
    })
}

/// `RE_KNOWN_WIFIS.match(s)`: `^(\s*)KnownWifis:\s*(\S*)\s*$`, returning the
/// indent length and the inline value.
fn match_known_wifis(s: &str) -> Option<(usize, &str)> {
    let indent_end = s.find(|c: char| !is_space(c)).unwrap_or(s.len());
    let after = s[indent_end..].strip_prefix("KnownWifis:")?;
    let value = after.trim_start_matches(is_space);
    let inline_end = value.find(is_space).unwrap_or(value.len());
    let (inline, tail) = value.split_at(inline_end);
    tail.chars()
        .all(is_space)
        .then(|| (py_len(&s[..indent_end]), inline))
}

/// The shared rules with a key: `redact()` and `pseudonym()` from
/// `log_redactor`.
pub struct Redactor {
    rules: Rules,
    key: Vec<u8>,
}

impl Redactor {
    /// Compile the rules for `key` (the contents of the key file; any length
    /// works, as with Python's `hmac.new`).
    pub fn new(key: impl Into<Vec<u8>>) -> Redactor {
        Redactor {
            rules: Rules::new(),
            key: key.into(),
        }
    }

    /// A redactor for another key that shares these compiled rules.
    pub fn with_key(&self, key: impl Into<Vec<u8>>) -> Redactor {
        Redactor {
            rules: self.rules.clone(),
            key: key.into(),
        }
    }

    /// A fresh log filter that shares these compiled rules (its key comes
    /// from `key_source`, not from this redactor).
    pub fn log_redactor(&self, key_source: impl KeySource + 'static) -> LogRedactor {
        LogRedactor::with_rules(self.rules.clone(), key_source)
    }

    /// The key material.
    pub fn key(&self) -> &[u8] {
        &self.key
    }

    /// `pseudonym(kind, value, key)` with this redactor's key.
    pub fn pseudonym(&self, kind: &str, value: &str) -> String {
        pseudonym(kind, value, &self.key)
    }

    /// `redact(text, key)`: a whole-file pass with fresh block state.
    ///
    /// PARITY: the unquoted `ssid: value` rule (R4i) stops only at `\n`, so on
    /// a line ending in `\r\n`, `\r` or another terminator it swallows the
    /// terminator into the value and the line break disappears from the output.
    pub fn redact(&self, text: &str) -> Redacted {
        self.redact_with_state(text, &mut RedactionState::default())
    }

    /// `redact(text, key, state=state)`, for callers that feed one record at
    /// a time and need KnownWifis tracking to persist between them.
    pub fn redact_with_state(&self, text: &str, state: &mut RedactionState) -> Redacted {
        match self.rules.redact(text, &self.key, state, None) {
            Ok(redacted) => redacted,
            Err(RedactionCancelled) => unreachable!("no cancellation predicate was given"),
        }
    }

    /// `redact(text, key, state, cancelled)`: `cancelled` is polled on every
    /// 500th line of pass 1 and once per learned SSID in pass 2.
    pub fn redact_cancellable(
        &self,
        text: &str,
        state: Option<&mut RedactionState>,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> Result<Redacted, RedactionCancelled> {
        let mut fresh = RedactionState::default();
        let state = state.unwrap_or(&mut fresh);
        self.rules.redact(text, &self.key, state, Some(cancelled))
    }
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Redactor").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [7; 32];

    fn redact(text: &str) -> String {
        Redactor::new(KEY).redact(text).text
    }

    #[test]
    fn ipv6_lookahead_backtracking() {
        // The chain is followed by a word character: nothing matches.
        assert_eq!(redact("::abcd:1234x"), "::abcd:1234x");
        // A full 8-group address followed by `:9` still matches branch A.
        let out = redact("1:2:3:4:5:6:7:8:9");
        assert!(out.starts_with("[IPV6_") && out.ends_with("]:9"), "{out}");
        // Timestamps are not addresses (spec section 8.2).
        assert_eq!(redact("01:42:05.388326"), "01:42:05.388326");
    }

    #[test]
    fn timezone_backtracks_past_a_trailing_word_character() {
        assert_eq!(redact("America/Mexico_City-6x"), "America/*****-6x");
        // Backtracks to "Etc/GMT", which is allowlisted.
        assert_eq!(redact("Etc/GMT-6x"), "Etc/GMT-6x");
        assert_eq!(redact("Etc/GMT+12/Foo"), "Etc/*****");
        assert_eq!(redact("America/Foo1"), "America/Foo1");
        assert_eq!(redact("Europe/Berlin/Mitte9"), "Europe/*****/Mitte9");
    }

    #[test]
    fn credential_word_boundary_uses_cpython_word_class() {
        // `_` is a word character: no boundary before "password".
        assert_eq!(redact("my_password: x"), "my_password: x");
        // U+0301 is not a word character for CPython, so the boundary holds.
        assert_eq!(
            redact("e\u{301}password: x"),
            "e\u{301}password: [REDACTED]"
        );
        // CPython folds the dotless i into `i` under IGNORECASE.
        assert_eq!(redact("SECRET: x"), "SECRET: [REDACTED]");
        assert_eq!(redact("secr\u{131}t: x"), "secr\u{131}t: x");
        assert_eq!(redact("tok\u{212A}en: x"), "tok\u{212A}en: x");
        assert_eq!(redact("to\u{212A}en: x"), "to\u{212A}en: [REDACTED]");
        assert_eq!(redact("ap\u{131}key: x"), "ap\u{131}key: [REDACTED]");
    }

    #[test]
    fn connection_allowlist() {
        assert!(connection_allowlisted("lo"));
        assert!(connection_allowlisted("LO"));
        assert!(connection_allowlisted("usb0"));
        assert!(connection_allowlisted("u\u{17F}b12"));
        assert!(connection_allowlisted("eth\u{663}"));
        assert!(connection_allowlisted("W\u{130}RED CONNECTION 2"));
        assert!(!connection_allowlisted("eth"));
        assert!(!connection_allowlisted("lo0"));
    }

    #[test]
    fn already_redacted_accepts_one_trailing_newline() {
        assert!(is_already_redacted("[REDACTED]"));
        assert!(is_already_redacted("[REDACTED]\n"));
        assert!(!is_already_redacted("[REDACTED]\n\n"));
        assert!(is_already_redacted("[MAC_0123abcd]"));
        assert!(!is_already_redacted("[MAC_0123ABCD]"));
        assert!(!is_already_redacted("[TZ_0123abcd]"));
    }

    #[test]
    fn key_check() {
        assert!(check_key("/k", &[0; 32]).is_ok());
        let err = check_key("/k", b"too-short").unwrap_err();
        assert_eq!(
            err.to_string(),
            "redaction key at '/k' must be exactly 32 bytes"
        );
    }
}
