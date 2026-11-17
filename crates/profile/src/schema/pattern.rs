//! Python `re.search` patterns run with the `regex` crate.
//!
//! The syntax the schemas use is common to both engines except for `$`:
//! Python's (without `re.MULTILINE`) also matches just before a final
//! newline, so `"#abcdef\n"` matches `^#[0-9A-Fa-f]{6}$`. Outside a
//! character class an unescaped `$` becomes `\n?\z`; for a yes/no search
//! that is the same test. `\Z` (Python's end of string) becomes `\z`.

use regex::Regex;

pub(super) fn compile(pattern: &str) -> Result<Regex, String> {
    Regex::new(&translate(pattern)).map_err(|e| e.to_string())
}

fn translate(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut chars = pattern.chars().peekable();
    let mut in_class = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('Z') if !in_class => out.push_str("\\z"),
                Some(next) => {
                    out.push('\\');
                    out.push(next);
                }
                None => out.push('\\'),
            },
            '[' if !in_class => {
                in_class = true;
                out.push('[');
                // A `]` right after `[` or `[^` is a literal member.
                if chars.peek() == Some(&'^') {
                    out.push('^');
                    chars.next();
                }
                if chars.peek() == Some(&']') {
                    out.push_str("\\]");
                    chars.next();
                }
            }
            ']' if in_class => {
                in_class = false;
                out.push(']');
            }
            '$' if !in_class => out.push_str("(?:\\n?\\z)"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dollar_matches_before_a_final_newline() {
        let regex = compile("^#[0-9A-Fa-f]{6}$").expect("compiles");
        assert!(regex.is_match("#abcdef"));
        assert!(regex.is_match("#abcdef\n"));
        assert!(!regex.is_match("#abcdef\n\n"));
        assert!(!regex.is_match("#abcdefa"));
    }

    #[test]
    fn escaped_dollar_is_literal() {
        let regex = compile("^\\$").expect("compiles");
        assert!(regex.is_match("$x"));
        assert!(!regex.is_match("x$"));
    }
}
