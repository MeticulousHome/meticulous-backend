//! HTTP requests of the report module: the watcher's logs and status
//! (`_get_watcher_body`, `_fetch_watcher_text`) and the preflight probes
//! (`_probe_url`), which the Python makes with aiohttp.
//!
//! aiohttp ignores the proxy environment by default (`trust_env=False`), so
//! these clients do too. A failure is described the way the aiohttp
//! exception would be: its class name and its `str()`, which is what ends up
//! in `logs_while_reporting.txt` and in a probe's `error`.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// `PREFLIGHT_PROBE_TIMEOUT_SECONDS`.
pub const PREFLIGHT_PROBE_TIMEOUT_SECONDS: u64 = 5;
/// `iter_chunked(64 * 1024)`.
const CHUNK: usize = 64 * 1024;

/// An aiohttp (or asyncio) exception: `type(exc).__name__` and `str(exc)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpError {
    pub type_name: String,
    pub message: String,
}

impl HttpError {
    pub fn new(type_name: &str, message: impl Into<String>) -> HttpError {
        HttpError {
            type_name: type_name.to_string(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Why a watcher fetch did not produce text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The client that asked for the report disconnected (`CancelledError`).
    Cancelled,
    Failed(HttpError),
}

fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(inner) = source {
        text.push_str(": ");
        text.push_str(&inner.to_string());
        source = inner.source();
    }
    text
}

/// The aiohttp exception a reqwest failure corresponds to.
pub fn classify(error: &reqwest::Error, url: &str) -> HttpError {
    if error.is_timeout() {
        // asyncio.TimeoutError: its str() is empty.
        return HttpError::new("TimeoutError", "");
    }
    let parsed = url::Url::parse(url).ok();
    let host = parsed
        .as_ref()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_default();
    let port = parsed
        .as_ref()
        .and_then(url::Url::port_or_known_default)
        .map(|p| p.to_string())
        .unwrap_or_else(|| "None".into());
    let chain = error_chain(error);
    let lower = chain.to_lowercase();
    if error.is_connect() {
        let cannot =
            |detail: &str| format!("Cannot connect to host {host}:{port} ssl:default [{detail}]");
        if lower.contains("dns error") {
            let detail = chain.rsplit(": ").next().unwrap_or_default().to_string();
            return HttpError::new("ClientConnectorDNSError", cannot(&detail));
        }
        if lower.contains("certificate") {
            return HttpError::new("ClientConnectorCertificateError", cannot(&chain));
        }
        if lower.contains("tls") || lower.contains("handshake") {
            return HttpError::new("ClientConnectorSSLError", cannot(&chain));
        }
        let detail = if lower.contains("refused") {
            format!("Connect call failed ('{host}', {port})")
        } else {
            chain.rsplit(": ").next().unwrap_or_default().to_string()
        };
        return HttpError::new("ClientConnectorError", cannot(&detail));
    }
    if error.is_builder() {
        return HttpError::new("InvalidUrlClientError", url);
    }
    if error.is_body() || error.is_decode() {
        return HttpError::new("ClientPayloadError", "Response payload is not completed");
    }
    if error.is_request() {
        return HttpError::new("ServerDisconnectedError", "Server disconnected");
    }
    HttpError::new("ClientError", chain)
}

/// `aiohttp.ClientResponseError` of `raise_for_status=True`.
fn status_error(status: reqwest::StatusCode, url: &str) -> HttpError {
    HttpError::new(
        "ClientResponseError",
        format!(
            "{}, message={}, url={}",
            status.as_u16(),
            super::py::repr_str(status.canonical_reason().unwrap_or("")),
            super::py::repr_str(url)
        ),
    )
}

/// The newest `max_bytes` of a body read in chunks: whole chunks are
/// dropped from the front while more than `max_bytes` are kept (never the
/// last one), and a dropped head is replaced by a banner up to the first
/// newline of what is kept.
#[derive(Debug)]
pub struct Tail {
    max_bytes: i64,
    chunks: VecDeque<Vec<u8>>,
    kept: i64,
    dropped: i64,
}

impl Tail {
    pub fn new(max_bytes: i64) -> Tail {
        Tail {
            max_bytes,
            chunks: VecDeque::new(),
            kept: 0,
            dropped: 0,
        }
    }

    /// Add what the network delivered, in `iter_chunked` sized pieces.
    pub fn push(&mut self, bytes: &[u8]) {
        for piece in bytes.chunks(CHUNK) {
            self.chunks.push_back(piece.to_vec());
            self.kept += piece.len() as i64;
            while self.kept > self.max_bytes && self.chunks.len() > 1 {
                if let Some(head) = self.chunks.pop_front() {
                    self.kept -= head.len() as i64;
                    self.dropped += head.len() as i64;
                }
            }
        }
    }

    pub fn finish(self) -> Vec<u8> {
        let body: Vec<u8> = self.chunks.into_iter().flatten().collect();
        if self.dropped == 0 {
            return body;
        }
        let mut out = format!(
            "[meticulous-backend] machine logs truncated: dropped the oldest {} bytes, kept the newest {} bytes\n",
            self.dropped, self.kept
        )
        .into_bytes();
        match body.iter().position(|b| *b == b'\n') {
            Some(newline) => out.extend_from_slice(&body[newline + 1..]),
            None => out.extend_from_slice(&body),
        }
        out
    }
}

/// `_get_watcher_body(url, timeout_seconds, max_bytes)`: GET with a total
/// timeout, a non-2xx status is an error.
pub async fn get_body(
    client: &reqwest::Client,
    url: &str,
    timeout: Duration,
    max_bytes: Option<i64>,
) -> Result<Vec<u8>, HttpError> {
    let response = client
        .get(url)
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| classify(&e, url))?;
    let status = response.status();
    if !status.is_success() {
        return Err(status_error(status, url));
    }
    match max_bytes {
        None => Ok(response
            .bytes()
            .await
            .map_err(|e| classify(&e, url))?
            .to_vec()),
        Some(max_bytes) => {
            let mut tail = Tail::new(max_bytes);
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                tail.push(&chunk.map_err(|e| classify(&e, url))?);
            }
            Ok(tail.finish())
        }
    }
}

/// `_fetch_watcher_text`: the body decoded with `errors="replace"`. A
/// disconnect stops the request at once, which closes its connection.
pub async fn fetch_text(
    client: &reqwest::Client,
    url: &str,
    timeout: Duration,
    cancellation: Option<&CancellationToken>,
    max_bytes: Option<i64>,
) -> Result<String, FetchError> {
    let body = match cancellation {
        Some(token) => {
            if token.is_cancelled() {
                return Err(FetchError::Cancelled);
            }
            tokio::select! {
                biased;
                () = token.cancelled() => return Err(FetchError::Cancelled),
                body = get_body(client, url, timeout, max_bytes) => body,
            }
        }
        None => get_body(client, url, timeout, max_bytes).await,
    };
    body.map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .map_err(FetchError::Failed)
}

/// `_probe_url(url)`: an HTTPS HEAD without redirects; any answer means
/// reachable.
pub async fn probe(client: &reqwest::Client, url: &str) -> Value {
    if !has_netloc(url) {
        return json!({"reachable": false, "status": null, "latencyMs": null, "error": "INVALID_PROBE"});
    }
    let started = Instant::now();
    match client
        .head(url)
        .timeout(Duration::from_secs(PREFLIGHT_PROBE_TIMEOUT_SECONDS))
        .send()
        .await
    {
        Ok(response) => json!({
            "reachable": true,
            "status": response.status().as_u16(),
            "latencyMs": started.elapsed().as_millis() as i64,
            "error": null,
        }),
        Err(error) => json!({
            "reachable": false,
            "status": null,
            "latencyMs": null,
            "error": classify(&error, url).type_name,
        }),
    }
}

/// `urlsplit(url).scheme == "https" and urlsplit(url).netloc`.
fn has_netloc(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once(':') else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    let Some(rest) = rest.strip_prefix("//") else {
        return false;
    };
    let netloc = rest.split(['/', '?', '#']).next().unwrap_or("");
    !netloc.is_empty()
}

/// The clients: aiohttp's defaults for the watcher (redirects followed) and
/// the probes' `allow_redirects=False`, neither reading proxy variables.
pub fn build_client(follow_redirects: bool) -> Result<reqwest::Client, HttpError> {
    let policy = if follow_redirects {
        reqwest::redirect::Policy::limited(10)
    } else {
        reqwest::redirect::Policy::none()
    };
    reqwest::Client::builder()
        .no_proxy()
        .redirect(policy)
        .build()
        .map_err(|e| HttpError::new("ClientError", e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `_get_watcher_body`'s truncation: the oldest whole chunks go, the
    /// first partial line after them is replaced by the banner.
    #[test]
    fn tail_keeps_the_newest_chunks() {
        let mut tail = Tail::new(CHUNK as i64);
        let mut body = Vec::new();
        for i in 0..3 {
            let mut chunk = vec![b'a' + i; CHUNK - 1];
            chunk.push(b'\n');
            body.extend_from_slice(&chunk);
        }
        tail.push(&body);
        let out = tail.finish();
        let banner = format!(
            "[meticulous-backend] machine logs truncated: dropped the oldest {} bytes, kept the newest {} bytes\n",
            2 * CHUNK,
            CHUNK
        );
        assert!(out.starts_with(banner.as_bytes()));
        assert_eq!(out.len(), banner.len());

        let mut small = Tail::new(10);
        small.push(b"short");
        assert_eq!(small.finish(), b"short");
    }

    #[test]
    fn probe_targets_must_be_https_with_a_host() {
        assert!(has_netloc("https://example.com/x"));
        assert!(has_netloc("HTTPS://example.com"));
        assert!(!has_netloc("http://example.com"));
        assert!(!has_netloc("https://"));
        assert!(!has_netloc("https:/x"));
    }

    #[test]
    fn status_errors_read_like_aiohttp() {
        assert_eq!(
            status_error(reqwest::StatusCode::INTERNAL_SERVER_ERROR, "http://w/x").message,
            "500, message='Internal Server Error', url='http://w/x'"
        );
    }
}
