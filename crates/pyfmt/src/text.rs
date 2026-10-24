/// Python `str.strip("\r\n")`.
pub fn strip_crlf(s: &str) -> &str {
    s.trim_matches(|c| c == '\r' || c == '\n')
}

/// `urllib.parse.unquote`: decode `%XX` escapes as UTF-8, keep malformed
/// escapes as they are and replace invalid UTF-8 with U+FFFD.
pub fn unquote(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let decoded = bytes.get(i + 1).zip(bytes.get(i + 2)).and_then(|(hi, lo)| {
                let hi = (*hi as char).to_digit(16)?;
                let lo = (*lo as char).to_digit(16)?;
                u8::try_from(hi * 16 + lo).ok()
            });
            if let Some(byte) = decoded {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(b);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unquote_like_urllib() {
        assert_eq!(unquote("click%20to%20start"), "click to start");
        assert_eq!(unquote("caf%C3%A9"), "café");
        assert_eq!(unquote("100%"), "100%");
        assert_eq!(unquote("%zz"), "%zz");
        assert_eq!(unquote("%ff"), "\u{fffd}");
    }

    #[test]
    fn strip_only_line_endings() {
        assert_eq!(strip_crlf("\r\nidle\r\n"), "idle");
        assert_eq!(strip_crlf(" idle "), " idle ");
    }
}
