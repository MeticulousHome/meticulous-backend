//! `json.dumps(json.loads(text))`, written token by token from the text.
//!
//! The history API answers with every sample of the shots it lists:
//! megabytes of JSON read from shot files. Parsing them into
//! `serde_json::Value`s only to encode them again is most of that work, so
//! the samples are transcoded instead: each token is checked as CPython's
//! `json` scanner checks it and written as CPython's encoder writes the value
//! it reads. Shot files are `json.dumps` output already, so most of the text
//! comes out as it went in: it is copied in runs, and only what reads
//! differently (whitespace, a float spelled otherwise, a string to escape) is
//! rewritten.
//!
//! The output is CPython's, also where [`loads_lenient`](crate::loads_lenient)
//! and [`dumps`](crate::dumps) differ from it because a `serde_json::Value`
//! cannot hold what Python reads: the bare `NaN`, `Infinity` and `-Infinity`
//! tokens are written back as they are (not `null`), integers keep every
//! digit (not a float past 64 bits), `-0` is the integer `0`, a float beyond
//! the double range is `Infinity`, and a lone surrogate escape is kept
//! (`ensure_ascii`) instead of failing the document.

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde_json::Value;

use crate::float::write_repr_f64;
use crate::json::{DumpOpts, write_string, write_value};

/// How the streaming encoders write: `json.dumps` with its default
/// separators (`", "` and `": "`) and no indent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonStyle {
    /// `ensure_ascii`: everything outside printable ASCII as `\uXXXX`.
    pub ensure_ascii: bool,
    /// Tornado's `json_encode` on top: every `</` written as `<\/`, so the
    /// body can sit inside a script tag.
    pub escape_closing_tags: bool,
    /// `NaN`, `Infinity`, `-Infinity` and floats beyond the double range
    /// written as `0.0`, so a browser's `JSON.parse` takes the text.
    pub finite_only: bool,
}

impl JsonStyle {
    /// `json.dumps(obj)`.
    pub const DUMPS: JsonStyle = JsonStyle {
        ensure_ascii: true,
        escape_closing_tags: false,
        finite_only: false,
    };
    /// `json.dumps(obj, ensure_ascii=False)`, how shot files are written.
    pub const UNICODE: JsonStyle = JsonStyle {
        ensure_ascii: false,
        escape_closing_tags: false,
        finite_only: false,
    };
    /// Tornado's `json_encode(obj)`: what `RequestHandler.write(dict)` sends.
    pub const TORNADO: JsonStyle = JsonStyle {
        ensure_ascii: true,
        escape_closing_tags: true,
        finite_only: false,
    };

    fn special_bytes(self) -> &'static [bool; 256] {
        match (self.ensure_ascii, self.escape_closing_tags) {
            (false, false) => &UNICODE_SPECIAL,
            (true, false) => &ASCII_SPECIAL,
            (false, true) => &UNICODE_TAGS_SPECIAL,
            (true, true) => &ASCII_TAGS_SPECIAL,
        }
    }
}

/// The bytes a run of string text stops at: the closing quote, a backslash,
/// control characters (an error), and what the style writes escaped: DEL
/// and everything past ASCII for `ensure_ascii`, `/` for the closing tags.
const fn special_bytes(ensure_ascii: bool, slash: bool) -> [bool; 256] {
    let mut table = [false; 256];
    let mut b = 0;
    while b < 256 {
        table[b] = b < 0x20
            || b == b'"' as usize
            || b == b'\\' as usize
            || (ensure_ascii && b >= 0x7f)
            || (slash && b == b'/' as usize);
        b += 1;
    }
    table
}

static UNICODE_SPECIAL: [bool; 256] = special_bytes(false, false);
static ASCII_SPECIAL: [bool; 256] = special_bytes(true, false);
static UNICODE_TAGS_SPECIAL: [bool; 256] = special_bytes(false, true);
static ASCII_TAGS_SPECIAL: [bool; 256] = special_bytes(true, true);

/// `json.dumps(value)` in `style`, appended to `out`.
pub fn write_json(out: &mut String, value: &Value, style: JsonStyle) {
    let opts = DumpOpts {
        ensure_ascii: style.ensure_ascii,
        ..DumpOpts::default()
    };
    write_value(out, value, &opts, 0, style.escape_closing_tags);
}

/// Why a document was refused: where CPython's `json.loads` raises, or
/// where the encoder would.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscodeError {
    message: String,
}

impl TranscodeError {
    /// A `JSONDecodeError`, located like Python's: line, column and the
    /// character index of the byte offset `at`.
    fn decode(text: &str, message: &str, at: usize) -> TranscodeError {
        let before = text.get(..at).unwrap_or(text);
        let line = before.matches('\n').count() + 1;
        let line_start = before.rfind('\n').map_or(0, |i| i + 1);
        let column = before[line_start..].chars().count() + 1;
        let index = before.chars().count();
        TranscodeError {
            message: format!("{message}: line {line} column {column} (char {index})"),
        }
    }

    fn other(message: String) -> TranscodeError {
        TranscodeError { message }
    }
}

impl fmt::Display for TranscodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TranscodeError {}

/// `json.dumps(json.loads(text), ...)` in `style`, appended to `out`,
/// without building the document.
///
/// On an error `out` holds whatever was written before it.
///
/// Nesting deeper than 127 levels is refused, as
/// [`loads_lenient`](crate::loads_lenient) refuses it.
pub fn transcode(text: &str, style: JsonStyle, out: &mut String) -> Result<(), TranscodeError> {
    let start = out.len();
    let mut parser = Parser::new(text, style);
    parser.skip_whitespace();
    parser.write_value(out)?;
    parser.finish()?;
    parser.check_surrogates(&out[start..])
}

/// What [`transcode_members`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Members<const N: usize> {
    /// The document is an object: per key, the transcoded value of its last
    /// member (`dict` keeps the last of repeated keys), `None` when absent.
    Object([Option<String>; N]),
    /// The document is valid but not an object: its Python type name.
    NotObject(&'static str),
}

/// `json.dumps(json.loads(text).get(key), ...)` for each of `keys`: the
/// whole document is checked like [`transcode`] checks it, but only the
/// values of these members of the top-level object are written.
pub fn transcode_members<const N: usize>(
    text: &str,
    keys: [&str; N],
    style: JsonStyle,
) -> Result<Members<N>, TranscodeError> {
    let mut parser = Parser::new(text, style);
    let mut discard = String::new();
    parser.skip_whitespace();
    let start = parser.pos;
    if parser.peek() != Some(b'{') {
        parser.value::<false, false>(&mut discard)?;
        parser.finish()?;
        return Ok(Members::NotObject(python_type_name(
            &text[start..parser.pos],
        )));
    }
    // Keys are compared in their `ensure_ascii` encoding, which is
    // one-to-one, so `"d\u0061ta"` is `data` as it is in Python, and a key
    // holding a lone surrogate needs no special case.
    let targets: [String; N] = keys.map(|key| {
        let mut encoded = String::new();
        write_string(&mut encoded, key, true, false);
        encoded
    });
    let mut found: [Option<String>; N] = std::array::from_fn(|_| None);
    let mut key = String::new();
    parser.enter(Nesting::Object)?;
    parser.pos += 1;
    parser.skip_whitespace();
    if parser.peek() == Some(b'}') {
        parser.pos += 1;
    } else {
        loop {
            if parser.peek() != Some(b'"') {
                return Err(parser.error(
                    "Expecting property name enclosed in double quotes",
                    parser.pos,
                ));
            }
            key.clear();
            let style = std::mem::replace(&mut parser.style, JsonStyle::DUMPS);
            let result = parser.string::<true, false>(&mut key);
            parser.style = style;
            result?;
            parser.skip_whitespace();
            if parser.peek() != Some(b':') {
                return Err(parser.error("Expecting ':' delimiter", parser.pos));
            }
            parser.pos += 1;
            parser.skip_whitespace();
            match targets.iter().position(|target| *target == key) {
                Some(index) => {
                    let out = found[index].get_or_insert_with(String::new);
                    out.clear();
                    if matches!(parser.peek(), Some(b'[' | b'{')) {
                        out.reserve(text.len() - parser.pos);
                    }
                    parser.write_value(out)?;
                }
                None => parser.value::<false, false>(&mut discard)?,
            }
            parser.skip_whitespace();
            match parser.peek() {
                Some(b'}') => {
                    parser.pos += 1;
                    break;
                }
                Some(b',') => {
                    parser.pos += 1;
                    parser.skip_whitespace();
                }
                _ => return Err(parser.error("Expecting ',' delimiter", parser.pos)),
            }
        }
    }
    parser.depth -= 1;
    parser.finish()?;
    for written in found.iter().flatten() {
        parser.check_surrogates(written)?;
    }
    Ok(Members::Object(found))
}

/// The Python type of a valid document that is not an object.
fn python_type_name(token: &str) -> &'static str {
    match token.as_bytes().first() {
        Some(b'[') => "list",
        Some(b'"') => "str",
        Some(b't' | b'f') => "bool",
        Some(b'n') => "NoneType",
        _ if token.contains(['.', 'e', 'E', 'N', 'I']) => "float",
        _ => "int",
    }
}

/// The deepest nesting accepted: `serde_json`'s limit, so a document
/// [`loads_lenient`](crate::loads_lenient) reads is never refused here.
/// CPython's limit is its recursion limit, which depends on the version and
/// on the stack of the caller (3.12 reads a few thousand levels).
const MAX_DEPTH: usize = 127;

/// CPython refuses integers longer than this (`sys.get_int_max_str_digits()`).
const MAX_INT_DIGITS: usize = 4300;

#[derive(Clone, Copy)]
enum Nesting {
    Array,
    Object,
}

/// A member of an object being read: where its key is (in the output when
/// writing in full, in the text when copying) and where its value ends in
/// the output (writing in full).
#[derive(Clone, Copy)]
struct Member {
    key_start: usize,
    key_end: usize,
    value_end: usize,
}

/// The keys of one object, for spotting a repeated one: a 256-bit filter of
/// their hashes, then a hash set once the object grows past
/// [`FILTERED_KEYS`] members.
#[derive(Default)]
struct KeySet {
    filter: [u64; 4],
    hashes: Option<HashSet<u64>>,
}

const FILTERED_KEYS: usize = 48;

/// The parser, and what it writes.
///
/// It reads in one of three modes, chosen by two const parameters on each
/// method: checking only (`EMIT` false), writing everything to `out`
/// (`EMIT`), and copying (`EMIT` and `COPY`), where what comes out exactly
/// as it is in the text is not written at once: `out` is then always
/// followed by the text from `run` to the cursor, still to be copied.
/// [`Parser::flush`] copies it before anything rewritten is written.
struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    depth: usize,
    style: JsonStyle,
    /// Copying: where the text not yet copied to `out` starts.
    run: usize,
    /// The members of the objects being read, innermost last.
    members: Vec<Member>,
    /// A lone surrogate was written without `ensure_ascii`.
    lone_surrogates: bool,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str, style: JsonStyle) -> Parser<'a> {
        Parser {
            text,
            bytes: text.as_bytes(),
            pos: 0,
            depth: 0,
            style,
            run: 0,
            members: Vec::new(),
            lone_surrogates: false,
        }
    }

    /// The value at the cursor, written to `out` (copying).
    fn write_value(&mut self, out: &mut String) -> Result<(), TranscodeError> {
        self.run = self.pos;
        self.value::<true, true>(out)?;
        self.flush(out, self.pos);
        Ok(())
    }

    /// Fail like encoding the str to UTF-8 fails if a lone surrogate
    /// written without `ensure_ascii` is still in the output. Without
    /// `ensure_ascii` only control characters and surrogates are written as
    /// `\uXXXX` (text copied as it is holds no backslash), so an escape in
    /// the D800-DFFF range is one.
    fn check_surrogates(&self, written: &str) -> Result<(), TranscodeError> {
        if !self.lone_surrogates {
            return Ok(());
        }
        let bytes = written.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != b'\\' {
                i += 1;
                continue;
            }
            if bytes[i + 1..].starts_with(b"ud")
                && matches!(bytes.get(i + 3), Some(b'8'..=b'9' | b'a'..=b'f'))
            {
                return Err(TranscodeError::other(format!(
                    "'utf-8' codec can't encode character '{}': surrogates not allowed",
                    written.get(i..i + 6).unwrap_or_default()
                )));
            }
            i += 2;
        }
        Ok(())
    }

    fn error(&self, message: &str, at: usize) -> TranscodeError {
        TranscodeError::decode(self.text, message, at)
    }

    #[inline]
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    #[inline]
    fn skip_whitespace(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.peek() {
            self.pos += 1;
        }
    }

    /// Only whitespace may follow the document.
    fn finish(&mut self) -> Result<(), TranscodeError> {
        self.skip_whitespace();
        if self.pos == self.bytes.len() {
            Ok(())
        } else {
            Err(self.error("Extra data", self.pos))
        }
    }

    fn enter(&mut self, nesting: Nesting) -> Result<(), TranscodeError> {
        if self.depth >= MAX_DEPTH {
            let what = match nesting {
                Nesting::Array => "array",
                Nesting::Object => "object",
            };
            return Err(TranscodeError::other(format!(
                "maximum recursion depth exceeded while decoding a JSON {what} from a unicode string"
            )));
        }
        self.depth += 1;
        Ok(())
    }

    // What is written for the text read. Copying, text that comes out as it
    // is costs nothing; the rest flushes the run before it.

    /// Copying: write the text from `run` up to `at`.
    #[inline]
    fn flush(&mut self, out: &mut String, at: usize) {
        out.push_str(&self.text[self.run..at]);
        self.run = at;
    }

    /// The text `from..to` comes out as it is.
    #[inline]
    fn same<const EMIT: bool, const COPY: bool>(
        &mut self,
        out: &mut String,
        from: usize,
        to: usize,
    ) {
        if EMIT && !COPY {
            out.push_str(&self.text[from..to]);
        }
    }

    /// The text `from..to` comes out as `with`.
    #[inline]
    fn replace<const EMIT: bool, const COPY: bool>(
        &mut self,
        out: &mut String,
        from: usize,
        to: usize,
        with: &str,
    ) {
        if EMIT {
            if COPY {
                if self.bytes[from..to] == *with.as_bytes() {
                    return;
                }
                self.flush(out, from);
                self.run = to;
            }
            out.push_str(with);
        }
    }

    /// Whitespace that is not written: from `from` to the cursor.
    #[inline]
    fn omit<const EMIT: bool, const COPY: bool>(&mut self, out: &mut String, from: usize) {
        if EMIT && COPY && self.pos > from {
            self.replace::<EMIT, COPY>(out, from, self.pos, "");
        }
    }

    /// The value at the cursor (CPython's `scan_once`).
    fn value<const EMIT: bool, const COPY: bool>(
        &mut self,
        out: &mut String,
    ) -> Result<(), TranscodeError> {
        match self.peek() {
            Some(b'"') => self.string::<EMIT, COPY>(out).map(|_| ()),
            Some(b'{') => self.object::<EMIT, COPY>(out),
            Some(b'[') => self.array::<EMIT, COPY>(out),
            Some(b'n') => self.literal::<EMIT, COPY>(out, "null"),
            Some(b't') => self.literal::<EMIT, COPY>(out, "true"),
            Some(b'f') => self.literal::<EMIT, COPY>(out, "false"),
            // float('nan') and float('inf') are written as these tokens.
            Some(b'N') => self.non_finite::<EMIT, COPY>(out, "NaN"),
            Some(b'I') => self.non_finite::<EMIT, COPY>(out, "Infinity"),
            Some(b'-') if self.bytes[self.pos..].starts_with(b"-Infinity") => {
                self.non_finite::<EMIT, COPY>(out, "-Infinity")
            }
            Some(b'-' | b'0'..=b'9') => self.number::<EMIT, COPY>(out),
            _ => Err(self.error("Expecting value", self.pos)),
        }
    }

    fn literal<const EMIT: bool, const COPY: bool>(
        &mut self,
        out: &mut String,
        word: &'static str,
    ) -> Result<(), TranscodeError> {
        if !self.bytes[self.pos..].starts_with(word.as_bytes()) {
            return Err(self.error("Expecting value", self.pos));
        }
        self.pos += word.len();
        self.same::<EMIT, COPY>(out, self.pos - word.len(), self.pos);
        Ok(())
    }

    /// `NaN`, `Infinity` or `-Infinity`: written back, or as `0.0` for a
    /// `finite_only` style.
    fn non_finite<const EMIT: bool, const COPY: bool>(
        &mut self,
        out: &mut String,
        word: &'static str,
    ) -> Result<(), TranscodeError> {
        if !self.style.finite_only {
            return self.literal::<EMIT, COPY>(out, word);
        }
        if !self.bytes[self.pos..].starts_with(word.as_bytes()) {
            return Err(self.error("Expecting value", self.pos));
        }
        let start = self.pos;
        self.pos += word.len();
        self.replace::<EMIT, COPY>(out, start, self.pos, "0.0");
        Ok(())
    }

    fn array<const EMIT: bool, const COPY: bool>(
        &mut self,
        out: &mut String,
    ) -> Result<(), TranscodeError> {
        self.enter(Nesting::Array)?;
        self.pos += 1;
        self.same::<EMIT, COPY>(out, self.pos - 1, self.pos);
        let from = self.pos;
        self.skip_whitespace();
        self.omit::<EMIT, COPY>(out, from);
        if self.peek() != Some(b']') {
            loop {
                self.value::<EMIT, COPY>(out)?;
                let end = self.pos;
                self.skip_whitespace();
                match self.peek() {
                    Some(b']') => {
                        self.omit::<EMIT, COPY>(out, end);
                        break;
                    }
                    Some(b',') => {
                        self.pos += 1;
                        self.skip_whitespace();
                        self.replace::<EMIT, COPY>(out, end, self.pos, ", ");
                    }
                    _ => return Err(self.error("Expecting ',' delimiter", self.pos)),
                }
            }
        }
        self.pos += 1;
        self.same::<EMIT, COPY>(out, self.pos - 1, self.pos);
        self.depth -= 1;
        Ok(())
    }

    fn object<const EMIT: bool, const COPY: bool>(
        &mut self,
        out: &mut String,
    ) -> Result<(), TranscodeError> {
        self.enter(Nesting::Object)?;
        let open = self.pos;
        // Writing in full: where the object starts in `out`. Copying: the
        // state to start over from if the object repeats a key.
        let (open_out, open_run) = (out.len(), self.run);
        self.pos += 1;
        self.same::<EMIT, COPY>(out, open, open + 1);
        let from = self.pos;
        self.skip_whitespace();
        self.omit::<EMIT, COPY>(out, from);
        let first = self.members.len();
        let mut keys = KeySet::default();
        let mut repeats = false;
        if self.peek() != Some(b'}') {
            loop {
                if self.peek() != Some(b'"') {
                    return Err(self.error(
                        "Expecting property name enclosed in double quotes",
                        self.pos,
                    ));
                }
                let (key_text, key_out) = (self.pos, out.len());
                let copied = self.string::<EMIT, COPY>(out)?;
                let key_end = self.pos;
                if EMIT && COPY {
                    // Keys without escapes are equal when their text is (a
                    // key copied as it is has none). A key with an escape, or
                    // a repeated key, is rare enough to write the whole
                    // object in full instead.
                    let key = key_text..key_end;
                    if (!copied && self.bytes[key.clone()].contains(&b'\\'))
                        || self.repeats_key(self.bytes, first, key.clone(), &mut keys)
                    {
                        return self.object_again(out, open, open_out, open_run, first);
                    }
                    self.members.push(Member {
                        key_start: key.start,
                        key_end: key.end,
                        value_end: 0,
                    });
                } else if EMIT && !repeats {
                    // Written keys are equal when their decoded strings are:
                    // the encoding is one-to-one.
                    let key = key_out..out.len();
                    repeats = self.repeats_key(out.as_bytes(), first, key, &mut keys);
                }
                let key_out_end = out.len();
                self.skip_whitespace();
                if self.peek() != Some(b':') {
                    return Err(self.error("Expecting ':' delimiter", self.pos));
                }
                self.pos += 1;
                self.skip_whitespace();
                self.replace::<EMIT, COPY>(out, key_end, self.pos, ": ");
                self.value::<EMIT, COPY>(out)?;
                if EMIT && !COPY {
                    self.members.push(Member {
                        key_start: key_out,
                        key_end: key_out_end,
                        value_end: out.len(),
                    });
                }
                let end = self.pos;
                self.skip_whitespace();
                match self.peek() {
                    Some(b'}') => {
                        self.omit::<EMIT, COPY>(out, end);
                        break;
                    }
                    Some(b',') => {
                        self.pos += 1;
                        self.skip_whitespace();
                        self.replace::<EMIT, COPY>(out, end, self.pos, ", ");
                    }
                    _ => return Err(self.error("Expecting ',' delimiter", self.pos)),
                }
            }
        }
        self.pos += 1;
        self.same::<EMIT, COPY>(out, self.pos - 1, self.pos);
        if EMIT && !COPY && repeats {
            self.merge_repeated_keys(out, open_out, first);
        }
        self.members.truncate(first);
        self.depth -= 1;
        Ok(())
    }

    /// Copying, the object at `open` turned out to need rewriting: back to
    /// the state when it opened, then written in full.
    fn object_again(
        &mut self,
        out: &mut String,
        open: usize,
        open_out: usize,
        open_run: usize,
        first: usize,
    ) -> Result<(), TranscodeError> {
        out.truncate(open_out);
        self.run = open_run;
        self.members.truncate(first);
        self.depth -= 1;
        self.flush(out, open);
        self.pos = open;
        self.object::<true, false>(out)?;
        self.run = self.pos;
        Ok(())
    }

    /// Whether the key at `key` in `haystack` (the text, or the output)
    /// repeats one of the object's earlier keys, found at the same kind of
    /// place.
    fn repeats_key(
        &self,
        haystack: &[u8],
        first: usize,
        key: std::ops::Range<usize>,
        keys: &mut KeySet,
    ) -> bool {
        let new = &haystack[key];
        let hash = key_hash(new);
        let earlier = &self.members[first..];
        let same = |member: &Member| haystack[member.key_start..member.key_end] == *new;
        if earlier.len() >= FILTERED_KEYS {
            let hashes = keys.hashes.get_or_insert_with(|| {
                earlier
                    .iter()
                    .map(|member| key_hash(&haystack[member.key_start..member.key_end]))
                    .collect()
            });
            // A new hash is a new key; a known one is checked exactly.
            return !hashes.insert(hash) && earlier.iter().any(same);
        }
        let bit = (hash >> 56) as usize;
        let (word, flag) = (bit / 64, 1u64 << (bit % 64));
        let maybe = keys.filter[word] & flag != 0;
        keys.filter[word] |= flag;
        maybe && earlier.iter().any(same)
    }

    /// Rewrite the object written from `open` the way a `dict` holds it:
    /// each key once, where it first appeared, with its last value.
    fn merge_repeated_keys(&self, out: &mut String, open: usize, first: usize) {
        let members = &self.members[first..];
        let key = |member: &Member| &out[member.key_start..member.key_end];
        let mut last: HashMap<&str, usize> = HashMap::new();
        for (index, member) in members.iter().enumerate() {
            last.insert(key(member), index);
        }
        let mut merged = String::with_capacity(out.len() - open);
        merged.push('{');
        let mut written = HashSet::new();
        for member in members {
            let name = key(member);
            if !written.insert(name) {
                continue;
            }
            let value = &members[last.get(name).copied().unwrap_or_default()];
            if written.len() > 1 {
                merged.push_str(", ");
            }
            merged.push_str(name);
            merged.push_str(": ");
            merged.push_str(&out[value.key_end + 2..value.value_end]);
        }
        merged.push('}');
        out.truncate(open);
        out.push_str(&merged);
    }

    /// The string at the cursor (CPython's `scanstring` with `strict=True`),
    /// written escaped for the style: runs of text that need no escape are
    /// copied as they are. Copying, returns whether the whole string was
    /// (it then holds no backslash).
    fn string<const EMIT: bool, const COPY: bool>(
        &mut self,
        out: &mut String,
    ) -> Result<bool, TranscodeError> {
        let begin = self.pos;
        let bytes = self.bytes;
        // Checking alone only stops where the input matters.
        let special = if EMIT {
            self.style.special_bytes()
        } else {
            &UNICODE_SPECIAL
        };
        let mut pos = begin + 1;
        if EMIT && COPY {
            while let Some(&b) = bytes.get(pos) {
                if special[usize::from(b)] {
                    break;
                }
                pos += 1;
            }
            if bytes.get(pos) == Some(&b'"') {
                self.pos = pos + 1;
                return Ok(true);
            }
            // Something to escape (or an error): the string written in full.
            self.flush(out, begin);
            self.string::<true, false>(out)?;
            self.run = self.pos;
            return Ok(false);
        }
        let mut run = pos;
        if EMIT {
            out.push('"');
        }
        loop {
            while let Some(&b) = bytes.get(pos) {
                if special[usize::from(b)] {
                    break;
                }
                pos += 1;
            }
            let Some(&b) = bytes.get(pos) else {
                return Err(self.error("Unterminated string starting at", begin));
            };
            // `run..pos` ends before an ASCII byte or a character's first
            // byte, so it is whole characters.
            if EMIT {
                out.push_str(&self.text[run..pos]);
            }
            match b {
                b'"' => {
                    if EMIT {
                        out.push('"');
                    }
                    self.pos = pos + 1;
                    return Ok(false);
                }
                b'\\' => pos = self.escape::<EMIT>(pos, begin, out)?,
                0..=0x1f => return Err(self.error("Invalid control character at", pos)),
                // Only stops here when writing: for the closing tags, and DEL
                // or a character past ASCII under `ensure_ascii`.
                b'/' => {
                    if EMIT {
                        self.write_slash(out);
                    }
                    pos += 1;
                }
                _ => {
                    let c = self.text[pos..].chars().next().unwrap_or_default();
                    if EMIT {
                        self.write_char(out, c);
                    }
                    pos += c.len_utf8().max(1);
                }
            }
            run = pos;
        }
    }

    /// The escape whose backslash is at `at`, in the string that opened at
    /// `begin`; returns the position after it.
    fn escape<const EMIT: bool>(
        &mut self,
        at: usize,
        begin: usize,
        out: &mut String,
    ) -> Result<usize, TranscodeError> {
        let len = self.bytes.len();
        let Some(&kind) = self.bytes.get(at + 1) else {
            return Err(self.error("Unterminated string starting at", begin));
        };
        let short = match kind {
            b'"' => '"',
            b'\\' => '\\',
            b'/' => '/',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => '\0',
            _ => return Err(self.error("Invalid \\escape", at)),
        };
        if kind != b'u' {
            if EMIT {
                self.write_char(out, short);
            }
            return Ok(at + 2);
        }
        // `\uXXXX`. As in CPython's scanner, a high surrogate takes a
        // following `\uXXXX` low surrogate with it and stays alone otherwise,
        // and the escape must not end the text.
        let mut end = at + 6;
        if end >= len {
            return Err(self.error("Invalid \\uXXXX escape", at + 1));
        }
        let Some(mut code) = self.hex4(at + 2) else {
            return Err(self.error("Invalid \\uXXXX escape", at + 1));
        };
        if (0xd800..0xdc00).contains(&code)
            && end + 6 < len
            && self.bytes[end] == b'\\'
            && self.bytes[end + 1] == b'u'
        {
            let Some(low) = self.hex4(end + 2) else {
                return Err(self.error("Invalid \\uXXXX escape", end + 1));
            };
            if (0xdc00..0xe000).contains(&low) {
                code = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
                end += 6;
            }
        }
        if EMIT {
            match char::from_u32(code) {
                Some(c) => self.write_char(out, c),
                None => {
                    // Python holds the surrogate in a str. Without
                    // `ensure_ascii` the str cannot be encoded, which only
                    // matters if a repeated key does not drop it: written
                    // escaped for now, see `check_surrogates`.
                    push_unicode_escape(out, code);
                    self.lone_surrogates |= !self.style.ensure_ascii;
                }
            }
        }
        Ok(end)
    }

    fn hex4(&self, at: usize) -> Option<u32> {
        let digits = self.bytes.get(at..at + 4)?;
        let mut value = 0;
        for &digit in digits {
            let nibble = match digit {
                b'0'..=b'9' => digit - b'0',
                b'a'..=b'f' => digit - b'a' + 10,
                b'A'..=b'F' => digit - b'A' + 10,
                _ => return None,
            };
            value = value << 4 | u32::from(nibble);
        }
        Some(value)
    }

    /// One character of a string, as `json.dumps` escapes it.
    fn write_char(&self, out: &mut String, c: char) {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\0'..='\u{1f}' => push_unicode_escape(out, u32::from(c)),
            '/' => self.write_slash(out),
            c if self.style.ensure_ascii && c >= '\u{7f}' => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    push_unicode_escape(out, u32::from(*unit));
                }
            }
            c => out.push(c),
        }
    }

    /// `/`, which tornado escapes after `<`. The `<` before it is always
    /// written as it is, in the same string, so the output tells.
    fn write_slash(&self, out: &mut String) {
        if self.style.escape_closing_tags && out.as_bytes().last() == Some(&b'<') {
            out.push_str("\\/");
        } else {
            out.push('/');
        }
    }

    /// The number at the cursor (CPython's `_match_number`): an `int`
    /// written with all its digits, or a `float` written as its `repr`.
    fn number<const EMIT: bool, const COPY: bool>(
        &mut self,
        out: &mut String,
    ) -> Result<(), TranscodeError> {
        let bytes = self.bytes;
        let start = self.pos;
        let digit = |at: usize| matches!(bytes.get(at), Some(b'0'..=b'9'));
        let mut pos = start;
        if bytes[pos] == b'-' {
            pos += 1;
        }
        let int_start = pos;
        match bytes.get(pos) {
            Some(b'0') => pos += 1,
            Some(b'1'..=b'9') => {
                pos += 1;
                while digit(pos) {
                    pos += 1;
                }
            }
            _ => return Err(self.error("Expecting value", start)),
        }
        let int_end = pos;
        let mut is_float = false;
        if bytes.get(pos) == Some(&b'.') && digit(pos + 1) {
            is_float = true;
            pos += 2;
            while digit(pos) {
                pos += 1;
            }
        }
        let fraction_end = pos;
        // An exponent without digits is not part of the number (and the
        // `e` left over is an error wherever it lands).
        if let Some(b'e' | b'E') = bytes.get(pos) {
            let mut exponent = pos + 1;
            if let Some(b'+' | b'-') = bytes.get(exponent) {
                exponent += 1;
            }
            let digits = exponent;
            while digit(exponent) {
                exponent += 1;
            }
            if exponent > digits {
                is_float = true;
                pos = exponent;
            }
        }
        self.pos = pos;
        let token = &self.text[start..pos];
        if !is_float {
            if int_end - int_start > MAX_INT_DIGITS {
                return Err(TranscodeError::other(format!(
                    "Exceeds the limit ({MAX_INT_DIGITS} digits) for integer string conversion: value has {} digits; use sys.set_int_max_str_digits() to increase the limit",
                    int_end - int_start
                )));
            }
            if token == "-0" {
                self.replace::<EMIT, COPY>(out, start, pos, "0");
            } else {
                self.same::<EMIT, COPY>(out, start, pos);
            }
            return Ok(());
        }
        if !EMIT {
            return Ok(());
        }
        if pos == fraction_end
            && is_float_repr(
                &bytes[int_start..int_end],
                &bytes[int_end + 1..fraction_end],
            )
        {
            self.same::<EMIT, COPY>(out, start, pos);
            return Ok(());
        }
        if COPY {
            self.flush(out, start);
            self.run = pos;
        }
        match token.parse::<f64>() {
            Ok(v) if v.is_infinite() && self.style.finite_only => out.push_str("0.0"),
            Ok(v) if v.is_infinite() => {
                out.push_str(if v > 0.0 { "Infinity" } else { "-Infinity" })
            }
            Ok(v) => write_repr_f64(out, v),
            // The JSON number grammar is a subset of Rust's.
            Err(_) => return Err(self.error("Expecting value", start)),
        }
        Ok(())
    }
}

/// Whether `<int>.<fraction>` (JSON digits, no exponent) is already the
/// `repr` of the float it reads as, so it can be copied.
///
/// A decimal of at most 15 significant digits (`DBL_DIG`) in the normal range
/// reads as a double that no other decimal of at most 15 digits reads as.
/// The shortest digits that read back as that double are therefore these,
/// without trailing zeros. What is left is the layout `repr` gives them in
/// positional notation, which it uses from `0.0001` up to `1e16` exclusive.
fn is_float_repr(int: &[u8], fraction: &[u8]) -> bool {
    if fraction.last() == Some(&b'0') {
        // Only `0.0` and integral values end in a zero: `100.0`.
        if fraction.len() != 1 {
            return false;
        }
        if int == b"0" {
            return true;
        }
        let significant = int.len() - int.iter().rev().take_while(|&&d| d == b'0').count();
        return int.len() <= 16 && significant <= 15;
    }
    if int == b"0" {
        let zeros = fraction.iter().take_while(|&&d| d == b'0').count();
        zeros <= 3 && fraction.len() - zeros <= 15
    } else {
        int.len() + fraction.len() <= 15
    }
}

fn push_unicode_escape(out: &mut String, unit: u32) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push_str("\\u");
    for shift in [12, 8, 4, 0] {
        out.push(char::from(HEX[((unit >> shift) & 0xf) as usize]));
    }
}

/// A cheap hash for spotting repeated keys, from the length and the first
/// and last eight bytes: a collision only costs an exact comparison.
fn key_hash(bytes: &[u8]) -> u64 {
    let n = bytes.len();
    let (mut head, mut tail) = ([0u8; 8], [0u8; 8]);
    if n >= 8 {
        head.copy_from_slice(&bytes[..8]);
        tail.copy_from_slice(&bytes[n - 8..]);
    } else {
        head[..n].copy_from_slice(bytes);
        tail = head;
    }
    let mixed = u64::from_le_bytes(head) ^ u64::from_le_bytes(tail).rotate_left(29) ^ n as u64;
    mixed.wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `finite_only` style writes every non-finite number as `0.0`.
    #[test]
    fn finite_only_writes_zero_for_non_finite_numbers() {
        let style = JsonStyle {
            finite_only: true,
            ..JsonStyle::TORNADO
        };
        let mut out = String::new();
        transcode(
            r#"{"a": NaN, "b": [Infinity, -Infinity, 1e400, -1e400], "c": 1.5}"#,
            style,
            &mut out,
        )
        .expect("valid");
        assert_eq!(out, r#"{"a": 0.0, "b": [0.0, 0.0, 0.0, 0.0], "c": 1.5}"#);
        let mut python = String::new();
        transcode("[NaN, -Infinity]", JsonStyle::TORNADO, &mut python).expect("valid");
        assert_eq!(python, "[NaN, -Infinity]");
    }

    fn dumps(text: &str) -> String {
        let mut out = String::new();
        transcode(text, JsonStyle::DUMPS, &mut out).expect("valid");
        out
    }

    fn unicode(text: &str) -> String {
        let mut out = String::new();
        transcode(text, JsonStyle::UNICODE, &mut out).expect("valid");
        out
    }

    fn tornado(text: &str) -> String {
        let mut out = String::new();
        transcode(text, JsonStyle::TORNADO, &mut out).expect("valid");
        out
    }

    /// Writing everything, without copying runs.
    fn in_full(text: &str, style: JsonStyle) -> Option<String> {
        let mut out = String::new();
        let mut parser = Parser::new(text, style);
        parser.skip_whitespace();
        parser.value::<true, false>(&mut out).ok()?;
        parser.finish().ok()?;
        parser.check_surrogates(&out).ok()?;
        Some(out)
    }

    fn refused(text: &str) -> String {
        let mut out = String::new();
        transcode(text, JsonStyle::DUMPS, &mut out)
            .expect_err(text)
            .to_string()
    }

    /// The expected values below are CPython 3.12's.
    #[test]
    fn numbers_like_cpython() {
        for (text, expected) in [
            ("-0", "0"),
            ("-0.0", "-0.0"),
            ("0.0", "0.0"),
            ("1E5", "100000.0"),
            ("1e+5", "100000.0"),
            ("0.5e-3", "0.0005"),
            ("1e400", "Infinity"),
            ("-1e400", "-Infinity"),
            ("1e-400", "0.0"),
            ("1e99999999999999999999", "Infinity"),
            ("0e99999999999", "0.0"),
            ("12345678901234567890", "12345678901234567890"),
            ("1.50", "1.5"),
            ("100.0", "100.0"),
            ("100.00", "100.0"),
            ("0.0001", "0.0001"),
            ("0.00001", "1e-05"),
            ("1000000000000000.0", "1000000000000000.0"),
            ("10000000000000000.0", "1e+16"),
            ("0.30000000000000004", "0.30000000000000004"),
            ("0.1000000000000000055511151231257827", "0.1"),
            ("[NaN, Infinity, -Infinity]", "[NaN, Infinity, -Infinity]"),
            ("[-0, 1.50,-0,0.0]", "[0, 1.5, 0, 0.0]"),
        ] {
            assert_eq!(dumps(text), expected, "{text}");
            assert_eq!(in_full(text, JsonStyle::DUMPS).as_deref(), Some(expected));
        }
    }

    #[test]
    fn strings_like_cpython() {
        let esc = |hex: &str| format!("{}u{hex}", '\\');
        assert_eq!(dumps(r#""\ud800""#), format!("\"{}\"", esc("d800")));
        assert_eq!(
            dumps(r#""\ud83d\ude00""#),
            format!("\"{}{}\"", esc("d83d"), esc("de00"))
        );
        assert_eq!(dumps(r#""\ud83d\u0041""#), format!("\"{}A\"", esc("d83d")));
        assert_eq!(unicode(r#""\ud83d\ude00""#), "\"\u{1f600}\"");
        assert_eq!(dumps("\"\u{7f}\""), format!("\"{}\"", esc("007f")));
        assert_eq!(unicode("\"\u{7f}\""), "\"\u{7f}\"");
        assert_eq!(dumps(r#""\/""#), r#""/""#);
        assert_eq!(
            dumps(r#""\u0000\b\f""#),
            format!("\"{}\\b\\f\"", esc("0000"))
        );
        assert_eq!(
            tornado(r#"{"</a>": "<\/b> \u003c/c a/b <</"}"#),
            r#"{"<\/a>": "<\/b> <\/c a/b <<\/"}"#
        );
        assert_eq!(
            dumps("[\"caf\u{e9}\", \"plain\", {\"k\u{e9}\": \"v\"}]"),
            format!(
                "[\"caf{}\", \"plain\", {{\"k{}\": \"v\"}}]",
                esc("00e9"),
                esc("00e9")
            )
        );
        let mut out = String::new();
        let error = transcode(r#""\ud800""#, JsonStyle::UNICODE, &mut out).expect_err("lone");
        assert!(error.to_string().contains("surrogates not allowed"));
        let mut out = String::new();
        assert!(transcode(r#"{"\udfff": 1}"#, JsonStyle::UNICODE, &mut out).is_err());
        // Only what is written out has to encode: a repeated key drops it.
        assert_eq!(
            unicode(r#"{"a": ["\ud800", {"\udbff": 2}], "b": "\\ud800", "a": 1}"#),
            r#"{"a": 1, "b": "\\ud800"}"#
        );
        assert_eq!(
            transcode_members(
                r#"{"data": "\ud800", "data": 1}"#,
                ["data"],
                JsonStyle::UNICODE
            ),
            Ok(Members::Object([Some("1".to_string())]))
        );
        assert!(transcode_members(r#"{"data": "\ud800"}"#, ["data"], JsonStyle::UNICODE).is_err());
    }

    #[test]
    fn duplicate_keys_keep_the_first_place_and_the_last_value() {
        assert_eq!(dumps(r#"{"a": 1, "b": 2, "a": 3}"#), r#"{"a": 3, "b": 2}"#);
        assert_eq!(
            dumps(r#"{"x": {"a": [1], "\u0061": {"b": 1, "b": 2}}, "y": 0, "x": 5}"#),
            r#"{"x": 5, "y": 0}"#
        );
        assert_eq!(
            dumps(r#"[{"a": {"b": 1, "b": 2}, "c": 3}]"#),
            r#"[{"a": {"b": 2}, "c": 3}]"#
        );
        // Copying, then starting the object over once a key repeats, after
        // rewritten text inside it and before it.
        assert_eq!(
            dumps("[1.50, {\"a\":1.50, \"x\": [ 2 ], \"a\": -0}, 3]"),
            "[1.5, {\"a\": 0, \"x\": [2]}, 3]"
        );
        let many: Vec<String> = (0..80).map(|i| format!("\"k{}\": {i}", i % 60)).collect();
        let expected: Vec<String> = (0..60)
            .map(|i| format!("\"k{i}\": {}", if i < 20 { i + 60 } else { i }))
            .collect();
        let text = format!("{{{}}}", many.join(", "));
        assert_eq!(dumps(&text), format!("{{{}}}", expected.join(", ")));
        assert_eq!(in_full(&text, JsonStyle::DUMPS), Some(dumps(&text)));
        let distinct: Vec<String> = (0..80).map(|i| format!("\"k{i}\": {i}")).collect();
        let text = format!("{{{}}}", distinct.join(", "));
        assert_eq!(dumps(&text), text);
    }

    #[test]
    fn refuses_what_cpython_refuses() {
        for (text, message) in [
            ("", "Expecting value: line 1 column 1 (char 0)"),
            ("1.", "Extra data: line 1 column 2 (char 1)"),
            (".5", "Expecting value: line 1 column 1 (char 0)"),
            ("01", "Extra data: line 1 column 2 (char 1)"),
            ("-", "Expecting value: line 1 column 1 (char 0)"),
            ("1e", "Extra data: line 1 column 2 (char 1)"),
            ("[1e]", "Expecting ',' delimiter: line 1 column 3 (char 2)"),
            (
                "\"a\u{1f}b\"",
                "Invalid control character at: line 1 column 3 (char 2)",
            ),
            ("[1,]", "Expecting value: line 1 column 4 (char 3)"),
            (
                "{\"a\":1,}",
                "Expecting property name enclosed in double quotes: line 1 column 8 (char 7)",
            ),
            ("nul", "Expecting value: line 1 column 1 (char 0)"),
            ("NaNx", "Extra data: line 1 column 4 (char 3)"),
            (
                "[NaN1]",
                "Expecting ',' delimiter: line 1 column 5 (char 4)",
            ),
            (
                r#""\ud83d\ude0""#,
                "Invalid \\uXXXX escape: line 1 column 9 (char 8)",
            ),
            ("\u{feff}{}", "Expecting value: line 1 column 1 (char 0)"),
            (
                "{\"a\" 1}",
                "Expecting ':' delimiter: line 1 column 6 (char 5)",
            ),
            (
                "\"abc",
                "Unterminated string starting at: line 1 column 1 (char 0)",
            ),
            ("\"\\x\"", "Invalid \\escape: line 1 column 2 (char 1)"),
        ] {
            assert_eq!(refused(text), message, "{text:?}");
        }
        assert_eq!(dumps(&"1".repeat(4300)), "1".repeat(4300));
        assert!(refused(&"1".repeat(4301)).starts_with("Exceeds the limit (4300 digits)"));
    }

    #[test]
    fn whitespace_is_dropped() {
        let text = " \t\n\r{ \"a\" :\n[ 1 ,2\t] , \"b\":{ }, \"c\": [ ] } \r\n";
        let expected = r#"{"a": [1, 2], "b": {}, "c": []}"#;
        assert_eq!(dumps(text), expected);
        assert_eq!(in_full(text, JsonStyle::DUMPS).as_deref(), Some(expected));
    }

    #[test]
    fn nesting_is_limited_like_loads_lenient() {
        let nested = |depth: usize| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
        assert!(crate::loads_lenient(&nested(MAX_DEPTH)).is_ok());
        assert!(crate::loads_lenient(&nested(MAX_DEPTH + 1)).is_err());
        assert_eq!(dumps(&nested(MAX_DEPTH)), nested(MAX_DEPTH));
        assert!(refused(&nested(MAX_DEPTH + 1)).starts_with("maximum recursion depth"));
        let objects = |depth: usize| format!("{}1{}", "{\"a\": ".repeat(depth), "}".repeat(depth));
        assert!(crate::loads_lenient(&objects(MAX_DEPTH)).is_ok());
        assert!(crate::loads_lenient(&objects(MAX_DEPTH + 1)).is_err());
        assert_eq!(dumps(&objects(MAX_DEPTH)), objects(MAX_DEPTH));
        assert!(refused(&objects(MAX_DEPTH + 1)).starts_with("maximum recursion depth"));
        // Starting an object over keeps the depth right.
        let repeated = format!(
            "{}{{\"a\": 1, \"a\": 2}}{}",
            "[".repeat(126),
            "]".repeat(126)
        );
        assert!(dumps(&repeated).contains("[{\"a\": 2}]"));
    }

    #[test]
    fn members_of_the_top_level_object() {
        let text = r#"{"time": 1.5, "data": [{"a": NaN}], "id": "x", "d\u0061ta": [1, 2.50], "push_to_brew_time": 0}"#;
        assert_eq!(
            transcode_members(
                text,
                ["data", "push_to_brew_time", "missing"],
                JsonStyle::DUMPS
            ),
            Ok(Members::Object([
                Some("[1, 2.5]".to_string()),
                Some("0".to_string()),
                None
            ]))
        );
        assert_eq!(
            transcode_members("{}", ["data"], JsonStyle::DUMPS),
            Ok(Members::Object([None]))
        );
        for (text, name) in [
            ("[1]", "list"),
            (" \"s\" ", "str"),
            ("true", "bool"),
            ("null", "NoneType"),
            ("1", "int"),
            ("-1.5e3", "float"),
            ("-Infinity", "float"),
            ("NaN", "float"),
        ] {
            assert_eq!(
                transcode_members(text, ["data"], JsonStyle::DUMPS),
                Ok(Members::NotObject(name)),
                "{text}"
            );
        }
        // Every member is checked, also those that are not written.
        assert!(
            transcode_members(r#"{"data": 1, "x": [1,]}"#, ["data"], JsonStyle::DUMPS).is_err()
        );
        assert!(transcode_members(r#"{"data": 1} x"#, ["data"], JsonStyle::DUMPS).is_err());
        assert!(transcode_members(r#"[1,]"#, ["data"], JsonStyle::DUMPS).is_err());
        let big = format!(r#"{{"x": {}, "data": 1}}"#, "1".repeat(4301));
        assert!(transcode_members(&big, ["data"], JsonStyle::DUMPS).is_err());
        // A lone surrogate is fine where nothing writes it.
        assert_eq!(
            transcode_members(
                r#"{"\ud800": "\udc00", "data": 1}"#,
                ["data"],
                JsonStyle::UNICODE
            ),
            Ok(Members::Object([Some("1".to_string())]))
        );
    }

    #[test]
    fn float_repr_shortcut() {
        for (int, fraction, copied) in [
            ("0", "0", true),
            ("0", "00", false),
            ("0", "5", true),
            ("0", "0001", true),
            ("0", "00001", false),
            ("0", "10", false),
            ("100", "0", true),
            ("1000000000000000", "0", true),
            ("10000000000000000", "0", false),
            ("999999999999999", "0", true),
            ("9999999999999999", "0", false),
            ("12345", "1234567891", true),
            ("12345", "12345678912", false),
            ("0", "123456789012345", true),
            ("0", "1234567890123456", false),
            ("0", "000123456789012345", true),
        ] {
            assert_eq!(
                is_float_repr(int.as_bytes(), fraction.as_bytes()),
                copied,
                "{int}.{fraction}"
            );
        }
    }
}
