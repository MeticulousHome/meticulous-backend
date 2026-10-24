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

    fn body_arguments(&self) -> Vec<(String, String)> {
        let form = self
            .header("content-type")
            .is_some_and(|ct| ct.starts_with("application/x-www-form-urlencoded"));
        if form {
            parse_qs(&String::from_utf8_lossy(&self.body))
        } else {
            Vec::new()
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
    fn json_errors_read_like_cpython() {
        assert_eq!(
            parse_python_json(b"").unwrap_err().message,
            "Expecting value: line 1 column 1 (char 0)"
        );
        assert!(parse_python_json(b"{\"a\": 1}").is_ok());
    }
}
