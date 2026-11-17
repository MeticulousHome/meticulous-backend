//! CPython's notion of characters, as the redaction rules see them.
//!
//! The rules are Python `re` patterns applied to `str`, and their output is a
//! contract (the same token in the watcher, the Python backend and here), so
//! "word character", "whitespace", "line break", case-insensitive letters and
//! `len()` all follow CPython 3.11 rather than Rust's Unicode definitions.

use std::fmt::Write as _;

use crate::pyunicode::{DECIMAL, PRINTABLE, WORD};

fn in_table(table: &[(u32, u32)], cp: u32) -> bool {
    table
        .binary_search_by(|&(lo, hi)| {
            if hi < cp {
                std::cmp::Ordering::Less
            } else if lo > cp {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// `re` `\w` on a `str` pattern: `str.isalnum()` or `_`.
pub(crate) fn is_word(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphanumeric() || c == '_';
    }
    in_table(WORD, c as u32)
}

/// `re` `\s`, which is also `str.isspace()` and what `str.strip()` removes.
/// Unlike Rust's `White_Space` it includes U+001C..U+001F.
pub(crate) fn is_space(c: char) -> bool {
    matches!(
        c as u32,
        0x09..=0x0D
            | 0x1C..=0x20
            | 0x85
            | 0xA0
            | 0x1680
            | 0x2000..=0x200A
            | 0x2028
            | 0x2029
            | 0x202F
            | 0x205F
            | 0x3000
    )
}

/// The body of a regex character class matching exactly [`is_space`].
pub(crate) const SPACE_CLASS: &str = r"\x09-\x0D\x1C-\x20\x{85}\x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}";

/// `re` `\d` on a `str` pattern (Unicode decimal digits).
pub(crate) fn is_decimal(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_digit();
    }
    in_table(DECIMAL, c as u32)
}

/// `[\w-]`, the boundary class of the literal sweeps.
pub(crate) fn is_word_or_dash(c: char) -> bool {
    c == '-' || is_word(c)
}

/// The code points `re.IGNORECASE` lets an ASCII letter of a pattern match,
/// beyond the letter itself: CPython folds `i` with U+0130/U+0131, `k` with
/// the Kelvin sign and `s` with the long s.
fn ignorecase_extras(lower: char) -> &'static [char] {
    match lower {
        'i' => &['\u{130}', '\u{131}'],
        'k' => &['\u{212A}'],
        's' => &['\u{17F}'],
        _ => &[],
    }
}

/// Whether `c` matches the ASCII letter `lower` under `re.IGNORECASE`.
pub(crate) fn ignorecase_eq(c: char, lower: char) -> bool {
    debug_assert!(lower.is_ascii_lowercase());
    c == lower || c == lower.to_ascii_uppercase() || ignorecase_extras(lower).contains(&c)
}

/// A regex fragment matching `word` the way `re.IGNORECASE` would.
///
/// The `regex` crate's `(?i)` uses Unicode simple case folding, which does
/// not fold `i` with U+0130/U+0131 the way CPython does, so case-insensitive
/// patterns spell every letter out as an explicit class instead.
pub(crate) fn ignorecase_pattern(word: &str) -> String {
    let mut out = String::new();
    for c in word.chars() {
        if c.is_ascii_alphabetic() {
            let lower = c.to_ascii_lowercase();
            out.push('[');
            out.push(lower);
            out.push(lower.to_ascii_uppercase());
            for extra in ignorecase_extras(lower) {
                let _ = write!(out, "\\x{{{:X}}}", *extra as u32);
            }
            out.push(']');
        } else {
            out.push_str(&regex::escape(&c.to_string()));
        }
    }
    out
}

/// `len(s)`: CPython counts code points.
pub(crate) fn py_len(s: &str) -> usize {
    s.chars().count()
}

/// `s.strip()` with no argument.
pub(crate) fn py_strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `s.strip("'\"")`.
pub(crate) fn strip_quotes(s: &str) -> &str {
    s.trim_matches(|c| c == '\'' || c == '"')
}

/// The character before byte offset `i`, as a lookbehind or `\b` sees it.
pub(crate) fn prev_char(text: &str, i: usize) -> Option<char> {
    text[..i].chars().next_back()
}

/// The character at byte offset `i`, as a lookahead or `\b` sees it.
pub(crate) fn next_char(text: &str, i: usize) -> Option<char> {
    text[i..].chars().next()
}

/// Byte length of the character at `i` (1 at the end, so scans terminate).
pub(crate) fn char_len_at(text: &str, i: usize) -> usize {
    next_char(text, i).map_or(1, char::len_utf8)
}

/// `str.splitlines(keepends=True)`.
///
/// CPython breaks lines at `\n`, `\r`, `\r\n`, `\v`, `\f`, U+001C, U+001D,
/// U+001E, U+0085, U+2028 and U+2029, not just at `\n`.
pub(crate) fn split_lines_keepends(text: &str) -> SplitLines<'_> {
    SplitLines { text, pos: 0 }
}

pub(crate) struct SplitLines<'a> {
    text: &'a str,
    pos: usize,
}

impl<'a> Iterator for SplitLines<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        let bytes = self.text.as_bytes();
        let start = self.pos;
        if start >= bytes.len() {
            return None;
        }
        let mut i = start;
        while let Some(&b) = bytes.get(i) {
            let sep_len = match b {
                b'\r' if bytes.get(i + 1) == Some(&b'\n') => 2,
                0x0A..=0x0D | 0x1C..=0x1E => 1,
                // U+0085 is C2 85; U+2028/U+2029 are E2 80 A8/A9.
                0xC2 if bytes.get(i + 1) == Some(&0x85) => 2,
                0xE2 if bytes.get(i + 1) == Some(&0x80)
                    && matches!(bytes.get(i + 2), Some(0xA8 | 0xA9)) =>
                {
                    3
                }
                _ => 0,
            };
            if sep_len > 0 {
                self.pos = i + sep_len;
                return Some(&self.text[start..self.pos]);
            }
            i += 1;
        }
        self.pos = bytes.len();
        Some(&self.text[start..])
    }
}

/// `repr(s)` for a `str`.
pub(crate) fn py_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        let cp = c as u32;
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            _ if c == quote => {
                out.push('\\');
                out.push(c);
            }
            _ if cp < 0x20 || cp == 0x7F => {
                let _ = write!(out, "\\x{cp:02x}");
            }
            _ if cp < 0x7F || in_table(PRINTABLE, cp) => out.push(c),
            _ if cp <= 0xFF => {
                let _ = write!(out, "\\x{cp:02x}");
            }
            _ => {
                out.push('\\');
                if cp <= 0xFFFF {
                    let _ = write!(out, "u{cp:04x}");
                } else {
                    let _ = write!(out, "U{cp:08x}");
                }
            }
        }
    }
    out.push(quote);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_class_follows_cpython_not_rust() {
        // U+00B2 SUPERSCRIPT TWO is numeric (No): \w for CPython, not for Rust.
        assert!(is_word('\u{B2}'));
        // U+0301 COMBINING ACUTE ACCENT (Mn): \w for Rust, not for CPython.
        assert!(!is_word('\u{301}'));
        assert!(is_word('\u{130}'));
        assert!(!is_word('-'));
    }

    #[test]
    fn space_class_includes_information_separators() {
        assert!(is_space('\u{1F}'));
        assert!(!'\u{1F}'.is_whitespace());
        assert!(!is_space('\u{200B}'));
    }

    #[test]
    fn splitlines_matches_cpython() {
        let parts: Vec<&str> =
            split_lines_keepends("a\r\nb\rc\nd\u{B}e\u{1C}f\u{85}g\u{2028}h\u{1F}i").collect();
        assert_eq!(
            parts,
            [
                "a\r\n",
                "b\r",
                "c\n",
                "d\u{B}",
                "e\u{1C}",
                "f\u{85}",
                "g\u{2028}",
                "h\u{1F}i"
            ]
        );
        assert_eq!(split_lines_keepends("").count(), 0);
        assert_eq!(split_lines_keepends("\n").collect::<Vec<_>>(), ["\n"]);
    }

    #[test]
    fn repr_matches_cpython() {
        assert_eq!(py_repr("wifi"), "'wifi'");
        assert_eq!(py_repr("it's"), "\"it's\"");
        assert_eq!(py_repr("a'b\"c"), "'a\\'b\"c'");
        assert_eq!(py_repr("tab\there\u{0}"), "'tab\\there\\x00'");
        assert_eq!(
            py_repr("caf\u{E9}\u{A0}\u{2028}"),
            "'caf\u{E9}\\xa0\\u2028'"
        );
    }

    #[test]
    fn ignorecase_pattern_spells_out_cpython_folds() {
        let re = regex::Regex::new(&format!("^{}$", ignorecase_pattern("ssid"))).unwrap();
        assert!(re.is_match("SSID"));
        assert!(re.is_match("\u{17F}s\u{130}d"));
        assert!(!re.is_match("ssjd"));
    }
}
