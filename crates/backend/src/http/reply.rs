use serde_json::Value;
use sha1::{Digest, Sha1};

use super::request::Request;
use super::static_files::StaticOptions;

const SERVER: &str = "TornadoServer/6.5.5";

/// The handler class a route maps to, which decides the default headers and
/// how the request is served.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// `api.base_handler.BaseHandler`: JSON content type, CORS, no caching.
    Base,
    /// A plain `tornado.web.RequestHandler`.
    Plain,
    /// `tornado.web.StaticFileHandler`.
    Static(StaticOptions),
    /// `tornado.web.RedirectHandler` (permanent).
    Redirect(String),
}

/// A response under construction, like a tornado `RequestHandler`'s.
#[derive(Debug, Clone)]
pub struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Reply {
    /// `RequestHandler.clear()`: tornado's headers, then the handler class's
    /// `set_default_headers()`.
    pub fn new(kind: Kind) -> Reply {
        let mut reply = Reply {
            status: 200,
            headers: vec![
                ("Server".into(), SERVER.into()),
                ("Content-Type".into(), "text/html; charset=UTF-8".into()),
                ("Date".into(), http_date(chrono::Utc::now())),
            ],
            body: Vec::new(),
        };
        if kind == Kind::Base {
            reply.base_handler_headers();
        }
        reply
    }

    /// A `BaseHandler` reply, the common case.
    pub fn base() -> Reply {
        Reply::new(Kind::Base)
    }

    /// `BaseHandler.set_default_headers()`.
    pub fn base_handler_headers(&mut self) {
        self.set_header("Access-Control-Allow-Origin", "*");
        self.set_header("Access-Control-Allow-Headers", "*");
        self.set_header("Access-Control-Expose-Headers", "*");
        self.set_header("Content-Type", "application/json");
        self.set_header(
            "Access-Control-Allow-Methods",
            "GET,POST,PUT,OPTIONS,DELETE",
        );
        self.set_header(
            "Access-Control-Allow-Headers",
            "content-type, authorization, x-authorized",
        );
        self.set_header("Cache-Control", "no-cache");
        self.set_header("Pragma", "no-cache");
        self.set_header("Expires", "0");
    }

    pub fn status_code(&self) -> u16 {
        self.status
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn status(mut self, code: u16) -> Reply {
        self.status = code;
        self
    }

    pub fn set_status(&mut self, code: u16) {
        self.status = code;
    }

    /// `set_header`: replaces any value the header had.
    pub fn set_header(&mut self, name: &str, value: impl Into<String>) {
        let value = value.into();
        match self
            .headers
            .iter_mut()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            Some(entry) => entry.1 = value,
            None => self.headers.push((name.to_string(), value)),
        }
    }

    pub fn with_header(mut self, name: &str, value: impl Into<String>) -> Reply {
        self.set_header(name, value);
        self
    }

    fn clear_header(&mut self, name: &str) {
        self.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
    }

    /// `write(dict)`: tornado's JSON encoding and its JSON content type.
    pub fn write_json(&mut self, value: &Value) {
        self.set_header("Content-Type", "application/json; charset=UTF-8");
        self.body
            .extend_from_slice(pyfmt::tornado_json_encode(value).as_bytes());
    }

    /// `write(json.dumps(value))`: a string body, the content type unchanged.
    pub fn write_dumps(&mut self, value: &Value) {
        self.write_str(&pyfmt::dumps_default(value));
    }

    /// `write(str)`.
    pub fn write_str(&mut self, text: &str) {
        self.body.extend_from_slice(text.as_bytes());
    }

    /// `write(bytes)`.
    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.body.extend_from_slice(bytes);
    }

    /// Builder form of [`Reply::write_json`].
    pub fn json(mut self, value: Value) -> Reply {
        self.write_json(&value);
        self
    }

    /// Builder form of [`Reply::write_dumps`].
    pub fn dumps(mut self, value: Value) -> Reply {
        self.write_dumps(&value);
        self
    }

    /// `report_error(code, error, details)` of `BaseHandler`.
    pub fn report_error(code: u16, error: &str, details: Value) -> Reply {
        Reply::base()
            .status(code)
            .json(serde_json::json!({"error": error, "details": details}))
    }

    /// `RequestHandler.finish()`: the automatic `Etag`/`304`, no
    /// representation headers on bodiless statuses, `Content-Length`.
    pub fn finish(mut self, req: &Request) -> Reply {
        let method = req.method.as_str();
        if self.status == 200
            && (method == "GET" || method == "HEAD")
            && self.header("Etag").is_none()
        {
            let etag = format!("\"{}\"", hex::encode(Sha1::digest(&self.body)));
            self.set_header("Etag", etag.clone());
            if etag_matches(&etag, req.header("If-None-Match").unwrap_or("")) {
                self.body.clear();
                self.status = 304;
            }
        }
        if matches!(self.status, 204 | 304) || (100..200).contains(&self.status) {
            if !self.body.is_empty() {
                tracing::error!("Cannot send body with {}", self.status);
                self.body.clear();
            }
            for header in ["Content-Encoding", "Content-Language", "Content-Type"] {
                self.clear_header(header);
            }
        } else if self.header("Content-Length").is_none() {
            let len = self.body.len().to_string();
            self.set_header("Content-Length", len);
        }
        self
    }

    /// `send_error(code)`: the handler class's headers and tornado's HTML page.
    pub fn error(kind: Kind, code: u16, req: &Request) -> Reply {
        let kind = match kind {
            Kind::Base => Kind::Base,
            _ => Kind::Plain,
        };
        let reason = reason_phrase(code);
        let mut reply = Reply::new(kind).status(code);
        reply.write_str(&format!(
            "<html><title>{code}: {reason}</title><body>{code}: {reason}</body></html>"
        ));
        reply.finish(req)
    }

    /// `RedirectHandler.get`: a permanent redirect that keeps the query.
    pub fn redirect(url: &str, req: &Request) -> Reply {
        let mut target = url.to_string();
        if !req.query.is_empty() {
            target.push(if target.contains('?') { '&' } else { '?' });
            target.push_str(&req.query);
        }
        Reply::new(Kind::Plain)
            .status(301)
            .with_header("Location", target)
            .finish(req)
    }

    pub fn into_axum(self) -> axum::response::Response {
        let mut builder = axum::response::Response::builder().status(self.status);
        for (name, value) in &self.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        builder
            .body(axum::body::Body::from(self.body))
            .unwrap_or_default()
    }
}

/// `RequestHandler.check_etag_header`.
fn etag_matches(computed: &str, if_none_match: &str) -> bool {
    let tags = regex_lite_etags(if_none_match);
    if tags.is_empty() {
        return false;
    }
    if tags[0] == "*" {
        return true;
    }
    let strip = |t: &str| t.strip_prefix("W/").unwrap_or(t).to_string();
    let computed = strip(computed);
    tags.iter().any(|t| strip(t) == computed)
}

/// `re.findall(r'\*|(?:W/)?"[^"]*"', header)`.
fn regex_lite_etags(header: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let bytes = header.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'*' {
            tags.push("*".to_string());
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i;
        if header[j..].starts_with("W/\"") {
            j += 2;
        }
        if bytes.get(j) == Some(&b'"')
            && let Some(end) = header[j + 1..].find('"')
        {
            let close = j + 1 + end;
            tags.push(header[start..=close].to_string());
            i = close + 1;
            continue;
        }
        i += 1;
    }
    tags
}

pub fn http_date(time: chrono::DateTime<chrono::Utc>) -> String {
    time.format("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

/// `http.client.responses`, the reason phrases tornado puts in error pages.
pub fn reason_phrase(code: u16) -> &'static str {
    match code {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        206 => "Partial Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        410 => "Gone",
        413 => "Request Entity Too Large",
        416 => "Requested Range Not Satisfiable",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etag_header_parsing() {
        assert!(etag_matches("\"abc\"", "\"abc\""));
        assert!(etag_matches("\"abc\"", "W/\"abc\""));
        assert!(etag_matches("\"abc\"", "\"x\", \"abc\""));
        assert!(etag_matches("\"abc\"", "*"));
        assert!(!etag_matches("\"abc\"", "\"abd\""));
        assert!(!etag_matches("\"abc\"", ""));
    }
}
