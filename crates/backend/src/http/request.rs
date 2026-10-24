use std::net::SocketAddr;

use bytes::Bytes;
use http::HeaderMap;
use serde_json::Value;

/// tornado's default `max_body_size`.
const MAX_BODY_BYTES: usize = 100 * 1024 * 1024;

/// A request as a tornado handler sees it.
#[derive(Debug, Clone)]
pub struct Request {
    pub method: http::Method,
    /// Path and query as sent.
    pub uri: String,
    /// The path as sent, still percent-encoded: tornado matches routes on it.
    pub path: String,
    pub query: String,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub remote_ip: String,
    /// The route's groups, percent-decoded; `None` for a group that did not
    /// take part in the match.
    pub path_args: Vec<Option<String>>,
}

/// Why a body is not the JSON the handler wanted, worded like CPython's
/// `json.JSONDecodeError`.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct JsonError {
    pub message: String,
}

/// A query or form argument whose bytes are not UTF-8.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Invalid unicode in argument")]
pub struct InvalidUnicode;

impl Request {
    pub async fn from_axum(
        request: axum::extract::Request,
        remote: Option<SocketAddr>,
    ) -> Result<Request, http::StatusCode> {
        let (parts, body) = request.into_parts();
        let body = axum::body::to_bytes(body, MAX_BODY_BYTES)
            .await
            .map_err(|_| http::StatusCode::PAYLOAD_TOO_LARGE)?;
        let path = parts.uri.path().to_string();
        let query = parts.uri.query().unwrap_or("").to_string();
        let uri = parts
            .uri
            .path_and_query()
            .map(|pq| pq.as_str().to_string())
            .unwrap_or_else(|| path.clone());
        Ok(Request {
            method: parts.method,
            uri,
            path,
            query,
            headers: parts.headers,
            body,
            remote_ip: remote
                .map(|a| a.ip().to_string())
                .unwrap_or_else(|| "127.0.0.1".into()),
            path_args: Vec::new(),
        })
    }

    /// A request built in tests.
    pub fn synthetic(method: http::Method, uri: &str, body: impl Into<Bytes>) -> Request {
        let (path, query) = uri.split_once('?').unwrap_or((uri, ""));
        Request {
            method,
            uri: uri.to_string(),
            path: path.to_string(),
            query: query.to_string(),
            headers: HeaderMap::new(),
            body: body.into(),
            remote_ip: "127.0.0.1".into(),
            path_args: Vec::new(),
        }
    }

    /// Positional path argument `i` (`None` when absent or unmatched).
    pub fn arg(&self, i: usize) -> Option<&str> {
        self.path_args.get(i).and_then(|a| a.as_deref())
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    pub fn query_arguments(&self) -> Vec<(String, String)> {
        parse_qs(&self.query)
    }

    fn is_form(&self) -> bool {
        self.header("content-type")
            .is_some_and(|ct| ct.starts_with("application/x-www-form-urlencoded"))
    }

    fn body_arguments(&self) -> Vec<(String, String)> {
        if self.is_form() {
            parse_qs(&String::from_utf8_lossy(&self.body))
        } else {
            Vec::new()
        }
    }

    /// `get_argument(name)` decoded the way tornado's `decode_argument`
    /// does it: a value that is not UTF-8 is an `HTTPError(400)` there,
    /// `Err` here.
    pub fn get_argument_checked(&self, name: &str) -> Result<Option<String>, InvalidUnicode> {
        let mut all = parse_qs_bytes(&self.query);
        if self.is_form() {
            all.extend(parse_qs_bytes(&String::from_utf8_lossy(&self.body)));
        }
        match all.into_iter().rev().find(|(k, _)| k == name) {
            None => Ok(None),
            Some((_, raw)) => String::from_utf8(raw)
                .map(|v| Some(v.trim().to_string()))
                .map_err(|_| InvalidUnicode),
        }
    }

    /// `get_argument(name)`: the last value from the query or a form body,
    /// with surrounding whitespace stripped.
    pub fn get_argument(&self, name: &str) -> Option<String> {
        let mut all = self.query_arguments();
        all.extend(self.body_arguments());
        all.into_iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.trim().to_string())
    }

    /// `get_query_argument(name)`: the last value from the query only.
    pub fn get_query_argument(&self, name: &str) -> Option<String> {
        self.get_query_arguments(name).pop()
    }

    /// `get_query_arguments(name)`.
    pub fn get_query_arguments(&self, name: &str) -> Vec<String> {
        self.query_arguments()
            .into_iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.trim().to_string())
            .collect()
    }

    pub fn get_argument_or(&self, name: &str, default: &str) -> String {
        self.get_argument(name)
            .unwrap_or_else(|| default.to_string())
    }

    /// `get_body_argument(name)`.
    pub fn get_body_argument(&self, name: &str) -> Option<String> {
        self.body_arguments()
            .into_iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.trim().to_string())
    }

    /// `json.loads(self.request.body)`.
    pub fn json(&self) -> Result<Value, JsonError> {
        parse_python_json(&self.body)
    }

    /// `self.request.files`: the file parts of a `multipart/form-data` body.
    pub async fn files(&self) -> Vec<UploadedFile> {
        let Some(boundary) = self
            .header("content-type")
            .and_then(|ct| multer::parse_boundary(ct).ok())
        else {
            return Vec::new();
        };
        let body = self.body.clone();
        let stream = futures::stream::once(async move { Ok::<Bytes, std::io::Error>(body) });
        let mut multipart = multer::Multipart::new(stream, boundary);
        let mut files = Vec::new();
        while let Ok(Some(field)) = multipart.next_field().await {
            let Some(filename) = field.file_name().map(str::to_string) else {
                continue;
            };
            let name = field.name().unwrap_or("").to_string();
            let content_type = field.content_type().map(|m| m.to_string());
            if let Ok(body) = field.bytes().await {
                files.push(UploadedFile {
                    field: name,
                    filename,
                    content_type,
                    body,
                });
            }
        }
        files
    }
}

/// One file of a multipart upload.
#[derive(Debug, Clone)]
pub struct UploadedFile {
    pub field: String,
    pub filename: String,
    pub content_type: Option<String>,
    pub body: Bytes,
}

/// `json.loads(bytes)` with CPython's error wording for the common cases.
pub fn parse_python_json(body: &[u8]) -> Result<Value, JsonError> {
    let text = std::str::from_utf8(body).map_err(|e| JsonError {
        message: format!("'utf-8' codec can't decode bytes: {e}"),
    })?;
    match pyfmt::loads_lenient(text) {
        Ok(value) => Ok(value),
        Err(error) => {
            if let Some((what, pos)) = cpython_json_error(text) {
                return Err(JsonError {
                    message: decode_error_message(text, what, pos),
                });
            }
            // A document CPython would take but serde does not: report
            // serde's position with the closest CPython wording.
            let line = error.line().max(1);
            let column = error.column().max(1);
            let offset = char_offset(text, line, column);
            let what = if text.trim().is_empty() || error.is_eof() {
                "Expecting value"
            } else if error.is_syntax() {
                match error.to_string() {
                    s if s.contains("key must be a string") => {
                        "Expecting property name enclosed in double quotes"
                    }
                    s if s.contains("trailing characters") => "Extra data",
                    s if s.contains("expected `,` or `}`") => "Expecting ',' delimiter",
                    s if s.contains("expected `:`") => "Expecting ':' delimiter",
                    _ => "Expecting value",
                }
            } else {
                "Expecting value"
            };
            Err(JsonError {
                message: format!("{what}: line {line} column {column} (char {offset})"),
            })
        }
    }
}

/// Where CPython's decoder (the `_json` C scanner of Python 3.11) gives up:
/// the message and the code-point index it reports, or `None` when it would
/// accept the document.
type JsonStop = (&'static str, usize);

fn cpython_json_error(text: &str) -> Option<JsonStop> {
    let s: Vec<char> = text.chars().collect();
    let start = skip_json_whitespace(&s, 0);
    match scan_json_value(&s, start) {
        Err(stop) => Some(stop),
        Ok(end) => {
            let end = skip_json_whitespace(&s, end);
            (end != s.len()).then_some(("Extra data", end))
        }
    }
}

/// `JSONDecodeError.__str__`: `"{msg}: line {lineno} column {colno} (char {pos})"`.
fn decode_error_message(text: &str, what: &str, pos: usize) -> String {
    let before: Vec<char> = text.chars().take(pos).collect();
    let line = before.iter().filter(|c| **c == '\n').count() + 1;
    let column = match before.iter().rposition(|c| *c == '\n') {
        Some(newline) => pos - newline,
        None => pos + 1,
    };
    format!("{what}: line {line} column {column} (char {pos})")
}

fn skip_json_whitespace(s: &[char], mut idx: usize) -> usize {
    while idx < s.len() && matches!(s[idx], ' ' | '\t' | '\n' | '\r') {
        idx += 1;
    }
    idx
}

fn json_word_at(s: &[char], idx: usize, word: &str) -> bool {
    let word: Vec<char> = word.chars().collect();
    s.get(idx..idx + word.len()) == Some(word.as_slice())
}

/// `scan_once_unicode`: the end of the value at `idx`. A value that cannot
/// start here is `StopIteration(idx)`, which `raw_decode` reports as
/// "Expecting value".
fn scan_json_value(s: &[char], idx: usize) -> Result<usize, JsonStop> {
    let Some(&c) = s.get(idx) else {
        return Err(("Expecting value", idx));
    };
    match c {
        '"' => scan_json_string(s, idx + 1),
        '{' => scan_json_object(s, idx + 1),
        '[' => scan_json_array(s, idx + 1),
        'n' if json_word_at(s, idx, "null") => Ok(idx + 4),
        't' if json_word_at(s, idx, "true") => Ok(idx + 4),
        'f' if json_word_at(s, idx, "false") => Ok(idx + 5),
        'N' if json_word_at(s, idx, "NaN") => Ok(idx + 3),
        'I' if json_word_at(s, idx, "Infinity") => Ok(idx + 8),
        '-' if json_word_at(s, idx, "-Infinity") => Ok(idx + 9),
        _ => scan_json_number(s, idx),
    }
}

/// `_match_number_unicode`: a fraction or exponent without digits is left
/// for the caller (then "Extra data" or a delimiter error).
fn scan_json_number(s: &[char], start: usize) -> Result<usize, JsonStop> {
    let len = s.len();
    let digit = |i: usize| s.get(i).is_some_and(char::is_ascii_digit);
    let mut idx = start;
    if s[idx] == '-' {
        idx += 1;
        if idx >= len {
            return Err(("Expecting value", start));
        }
    }
    match s[idx] {
        '1'..='9' => {
            idx += 1;
            while digit(idx) {
                idx += 1;
            }
        }
        '0' => idx += 1,
        _ => return Err(("Expecting value", start)),
    }
    if idx + 1 < len && s[idx] == '.' && digit(idx + 1) {
        idx += 2;
        while digit(idx) {
            idx += 1;
        }
    }
    if idx + 1 < len && matches!(s[idx], 'e' | 'E') {
        let e_start = idx;
        idx += 1;
        if idx + 1 < len && matches!(s[idx], '-' | '+') {
            idx += 1;
        }
        while digit(idx) {
            idx += 1;
        }
        if !digit(idx - 1) {
            idx = e_start;
        }
    }
    Ok(idx)
}

/// `scanstring_unicode` (strict): `end` is just past the opening quote.
fn scan_json_string(s: &[char], end: usize) -> Result<usize, JsonStop> {
    let len = s.len();
    let begin = end - 1;
    let hex4 = |from: usize| s[from..from + 4].iter().all(char::is_ascii_hexdigit);
    let mut next = end;
    loop {
        while next < len && s[next] != '"' && s[next] != '\\' {
            if u32::from(s[next]) <= 0x1f {
                return Err(("Invalid control character at", next));
            }
            next += 1;
        }
        if next == len {
            return Err(("Unterminated string starting at", begin));
        }
        if s[next] == '"' {
            return Ok(next + 1);
        }
        next += 1;
        if next == len {
            return Err(("Unterminated string starting at", begin));
        }
        if s[next] != 'u' {
            if !matches!(s[next], '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't') {
                return Err(("Invalid \\escape", next - 1));
            }
            next += 1;
            continue;
        }
        let u = next;
        let digits_end = u + 5;
        if digits_end >= len || !hex4(u + 1) {
            return Err(("Invalid \\uXXXX escape", u));
        }
        next = digits_end;
        let code = s[u + 1..digits_end]
            .iter()
            .fold(0u32, |acc, c| (acc << 4) | c.to_digit(16).unwrap_or(0));
        // A high surrogate takes the `\uXXXX` after it when one follows.
        if (0xD800..=0xDBFF).contains(&code)
            && next + 6 < len
            && s[next] == '\\'
            && s[next + 1] == 'u'
        {
            if !hex4(next + 2) {
                return Err(("Invalid \\uXXXX escape", next + 1));
            }
            let low = s[next + 2..next + 6]
                .iter()
                .fold(0u32, |acc, c| (acc << 4) | c.to_digit(16).unwrap_or(0));
            if (0xDC00..=0xDFFF).contains(&low) {
                next += 6;
            }
        }
    }
}

/// `_parse_object_unicode`: `idx` is just past the `{`.
fn scan_json_object(s: &[char], idx: usize) -> Result<usize, JsonStop> {
    let len = s.len();
    let mut idx = skip_json_whitespace(s, idx);
    if idx < len && s[idx] == '}' {
        return Ok(idx + 1);
    }
    loop {
        if idx >= len || s[idx] != '"' {
            return Err(("Expecting property name enclosed in double quotes", idx));
        }
        idx = skip_json_whitespace(s, scan_json_string(s, idx + 1)?);
        if idx >= len || s[idx] != ':' {
            return Err(("Expecting ':' delimiter", idx));
        }
        idx = skip_json_whitespace(s, idx + 1);
        idx = skip_json_whitespace(s, scan_json_value(s, idx)?);
        if idx < len && s[idx] == '}' {
            return Ok(idx + 1);
        }
        if idx >= len || s[idx] != ',' {
            return Err(("Expecting ',' delimiter", idx));
        }
        idx = skip_json_whitespace(s, idx + 1);
    }
}

/// `_parse_array_unicode`: `idx` is just past the `[`.
fn scan_json_array(s: &[char], idx: usize) -> Result<usize, JsonStop> {
    let len = s.len();
    let mut idx = skip_json_whitespace(s, idx);
    if idx < len && s[idx] == ']' {
        return Ok(idx + 1);
    }
    loop {
        idx = skip_json_whitespace(s, scan_json_value(s, idx)?);
        if idx < len && s[idx] == ']' {
            return Ok(idx + 1);
        }
        if idx >= len || s[idx] != ',' {
            return Err(("Expecting ',' delimiter", idx));
        }
        idx = skip_json_whitespace(s, idx + 1);
    }
}

fn char_offset(text: &str, line: usize, column: usize) -> usize {
    let mut offset = 0;
    for (i, l) in text.split('\n').enumerate() {
        if i + 1 == line {
            return offset + column.saturating_sub(1);
        }
        offset += l.chars().count() + 1;
    }
    offset
}

/// `urllib.parse.parse_qsl(query, keep_blank_values=True)`.
pub fn parse_qs(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (decode_form(k), decode_form(v))
        })
        .collect()
}

fn decode_form(s: &str) -> String {
    pyfmt::unquote(&s.replace('+', " "))
}

/// tornado's `parse_qs_bytes`: the values stay bytes until a handler asks
/// for one.
fn parse_qs_bytes(query: &str) -> Vec<(String, Vec<u8>)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (
                decode_form(k),
                pyfmt::unquote_to_bytes(&v.replace('+', " ")),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_like_tornado() {
        let req = Request::synthetic(
            http::Method::GET,
            "/x?full=true&a=1&a=2&b=&c=%20hi%20&d=a+b",
            "",
        );
        assert_eq!(req.get_argument("full").as_deref(), Some("true"));
        assert_eq!(req.get_argument("a").as_deref(), Some("2"));
        assert_eq!(req.get_argument("b").as_deref(), Some(""));
        assert_eq!(req.get_argument("c").as_deref(), Some("hi"));
        assert_eq!(req.get_argument("d").as_deref(), Some("a b"));
        assert_eq!(req.get_argument("missing"), None);
    }

    #[test]
    fn undecodable_arguments_like_tornado() {
        let req = Request::synthetic(http::Method::GET, "/x?f=%ff&g=caf%C3%A9+%20", "");
        assert_eq!(req.get_argument_checked("f"), Err(InvalidUnicode));
        assert_eq!(req.get_argument_checked("g"), Ok(Some("café".into())));
        assert_eq!(req.get_argument_checked("missing"), Ok(None));
    }

    #[test]
    fn json_errors_read_like_cpython() {
        assert_eq!(
            parse_python_json(b"").unwrap_err().message,
            "Expecting value: line 1 column 1 (char 0)"
        );
        assert!(parse_python_json(b"{\"a\": 1}").is_ok());
    }

    /// The messages and positions CPython 3.11's `json.loads` raises
    /// (printed by Python for each document).
    #[test]
    fn json_error_positions_follow_cpythons_scanner() {
        let cases = [
            ("   ", "Expecting value: line 1 column 4 (char 3)"),
            (
                "{",
                "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)",
            ),
            (
                "{not json",
                "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)",
            ),
            ("[", "Expecting value: line 1 column 2 (char 1)"),
            ("[1,", "Expecting value: line 1 column 4 (char 3)"),
            ("[1 2]", "Expecting ',' delimiter: line 1 column 4 (char 3)"),
            (
                "{\"a\"",
                "Expecting ':' delimiter: line 1 column 5 (char 4)",
            ),
            (
                "{\"a\" 1}",
                "Expecting ':' delimiter: line 1 column 6 (char 5)",
            ),
            ("{\"a\":", "Expecting value: line 1 column 6 (char 5)"),
            ("{\"a\": }", "Expecting value: line 1 column 7 (char 6)"),
            (
                "{\"a\": 1",
                "Expecting ',' delimiter: line 1 column 8 (char 7)",
            ),
            (
                "{\"a\": 1 \"b\": 2}",
                "Expecting ',' delimiter: line 1 column 9 (char 8)",
            ),
            (
                "{\"a\": 1,}",
                "Expecting property name enclosed in double quotes: line 1 column 9 (char 8)",
            ),
            ("[1,]", "Expecting value: line 1 column 4 (char 3)"),
            ("nul", "Expecting value: line 1 column 1 (char 0)"),
            ("tru", "Expecting value: line 1 column 1 (char 0)"),
            ("-", "Expecting value: line 1 column 1 (char 0)"),
            ("-x", "Expecting value: line 1 column 1 (char 0)"),
            ("01", "Extra data: line 1 column 2 (char 1)"),
            ("1.", "Extra data: line 1 column 2 (char 1)"),
            ("1e", "Extra data: line 1 column 2 (char 1)"),
            ("1.5e+", "Extra data: line 1 column 4 (char 3)"),
            (
                "\"abc",
                "Unterminated string starting at: line 1 column 1 (char 0)",
            ),
            (
                "\"a\\",
                "Unterminated string starting at: line 1 column 1 (char 0)",
            ),
            ("\"a\\x\"", "Invalid \\escape: line 1 column 3 (char 2)"),
            (
                "\"a\\u12\"",
                "Invalid \\uXXXX escape: line 1 column 4 (char 3)",
            ),
            (
                "\"a\\u12zz\"",
                "Invalid \\uXXXX escape: line 1 column 4 (char 3)",
            ),
            (
                "\"a\\u1234",
                "Invalid \\uXXXX escape: line 1 column 4 (char 3)",
            ),
            (
                "\"a\tb\"",
                "Invalid control character at: line 1 column 3 (char 2)",
            ),
            (
                "\"\\ud800\\uzzzz\"",
                "Invalid \\uXXXX escape: line 1 column 9 (char 8)",
            ),
            (
                "{\"a\": 1}\n{\"b\": 2}",
                "Extra data: line 2 column 1 (char 9)",
            ),
            (
                "{\n  \"a\": 1,\n  \"b\": ?\n}",
                "Expecting value: line 3 column 8 (char 19)",
            ),
            (
                "{\"\u{e9}\": x}",
                "Expecting value: line 1 column 7 (char 6)",
            ),
            (
                "[NaN, Infinity, -Infinity, x]",
                "Expecting value: line 1 column 28 (char 27)",
            ),
            (
                "{'a': 1}",
                "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)",
            ),
            ("}", "Expecting value: line 1 column 1 (char 0)"),
            ("[}", "Expecting value: line 1 column 2 (char 1)"),
            (
                "{\"a\": [1, 2}",
                "Expecting ',' delimiter: line 1 column 12 (char 11)",
            ),
        ];
        for (document, expected) in cases {
            assert_eq!(
                parse_python_json(document.as_bytes()).unwrap_err().message,
                expected,
                "{document:?}"
            );
        }
    }
}
