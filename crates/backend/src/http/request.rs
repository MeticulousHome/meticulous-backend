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
            if (error.is_eof() || text.trim().is_empty())
                && let Some((what, at)) = truncated_json(text)
            {
                return Err(JsonError {
                    message: python_json_position(what, text, at),
                });
            }
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

/// What CPython's decoder expected where a document that is valid so far
/// ends, and the character index it names: `{` wants a property name, `[1`
/// a `,`, `"ab` is an unterminated string. `None` when the text does not
/// read as a cut-off document.
fn truncated_json(text: &str) -> Option<(&'static str, usize)> {
    #[derive(Clone, Copy, PartialEq)]
    enum Want {
        Value,
        Key,
        Colon,
        CommaOrEnd,
    }
    let chars: Vec<char> = text.chars().collect();
    let mut containers: Vec<char> = Vec::new();
    let mut want = Want::Value;
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        match (want, c) {
            (_, ' ' | '\t' | '\n' | '\r') => i += 1,
            (Want::Value | Want::Key, '"') => {
                let start = i;
                i += 1;
                loop {
                    match chars.get(i) {
                        None => return Some(("Unterminated string starting at", start)),
                        Some('\\') => i += 2,
                        Some('"') => break,
                        Some(_) => i += 1,
                    }
                }
                i += 1;
                want = if want == Want::Key {
                    Want::Colon
                } else {
                    Want::CommaOrEnd
                };
            }
            (Want::Value, '{') => {
                containers.push('{');
                want = Want::Key;
                i += 1;
            }
            (Want::Value, '[') => {
                containers.push('[');
                i += 1;
            }
            (Want::Key | Want::CommaOrEnd, '}') | (Want::Value | Want::CommaOrEnd, ']') => {
                containers.pop()?;
                want = Want::CommaOrEnd;
                i += 1;
            }
            (Want::Colon, ':') => {
                want = Want::Value;
                i += 1;
            }
            (Want::CommaOrEnd, ',') => {
                want = match containers.last()? {
                    '{' => Want::Key,
                    _ => Want::Value,
                };
                i += 1;
            }
            (Want::Value, _) => {
                // A number or a literal, up to the next delimiter.
                let start = i;
                while chars
                    .get(i)
                    .is_some_and(|c| !matches!(c, ',' | ']' | '}' | ':' | ' ' | '\t' | '\n' | '\r'))
                {
                    i += 1;
                }
                let token: String = chars[start..i].iter().collect();
                if i == chars.len() && !complete_json_scalar(&token) {
                    return Some(("Expecting value", start));
                }
                want = Want::CommaOrEnd;
            }
            _ => return None,
        }
    }
    let what = match want {
        Want::Value => "Expecting value",
        Want::Key => "Expecting property name enclosed in double quotes",
        Want::Colon => "Expecting ':' delimiter",
        Want::CommaOrEnd => "Expecting ',' delimiter",
    };
    Some((what, chars.len()))
}

/// A number or a literal `json.loads` reads in full.
fn complete_json_scalar(token: &str) -> bool {
    static NUMBER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let number = NUMBER.get_or_init(|| {
        regex::Regex::new(r"^-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][-+]?[0-9]+)?$").expect("valid regex")
    });
    matches!(
        token,
        "true" | "false" | "null" | "NaN" | "Infinity" | "-Infinity"
    ) || number.is_match(token)
}

/// `JSONDecodeError`'s text: the message and where, as line, column and
/// character index.
fn python_json_position(what: &str, text: &str, at: usize) -> String {
    let before: Vec<char> = text.chars().take(at).collect();
    let line = before.iter().filter(|c| **c == '\n').count() + 1;
    let column = match before.iter().rposition(|c| *c == '\n') {
        Some(newline) => at - newline,
        None => at + 1,
    };
    format!("{what}: line {line} column {column} (char {at})")
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

    /// A body cut short names what CPython expected next (the texts are
    /// CPython 3.12's).
    #[test]
    fn truncated_json_errors_read_like_cpython() {
        for (body, message) in [
            ("   ", "Expecting value: line 1 column 4 (char 3)"),
            (
                "{",
                "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)",
            ),
            (
                "{ ",
                "Expecting property name enclosed in double quotes: line 1 column 3 (char 2)",
            ),
            (
                "{\"a\"",
                "Expecting ':' delimiter: line 1 column 5 (char 4)",
            ),
            ("{\"a\":", "Expecting value: line 1 column 6 (char 5)"),
            (
                "{\"a\": 1",
                "Expecting ',' delimiter: line 1 column 8 (char 7)",
            ),
            (
                "{\"a\": 1,",
                "Expecting property name enclosed in double quotes: line 1 column 9 (char 8)",
            ),
            ("[", "Expecting value: line 1 column 2 (char 1)"),
            ("[1", "Expecting ',' delimiter: line 1 column 3 (char 2)"),
            ("[1,", "Expecting value: line 1 column 4 (char 3)"),
            (
                "\"abc",
                "Unterminated string starting at: line 1 column 1 (char 0)",
            ),
            (
                "{\"ab",
                "Unterminated string starting at: line 1 column 2 (char 1)",
            ),
            ("[tr", "Expecting value: line 1 column 2 (char 1)"),
            ("[-", "Expecting value: line 1 column 2 (char 1)"),
            (
                "{\n  \"a\": [1,\n",
                "Expecting value: line 3 column 1 (char 13)",
            ),
        ] {
            assert_eq!(
                parse_python_json(body.as_bytes()).unwrap_err().message,
                message,
                "{body:?}"
            );
        }
    }
}
