//! Helpers for handler tests: the Rust counterpart of the Python suite's
//! `tornado.testing.AsyncHTTPTestCase` with temporary directories and the
//! hardware faked at its boundary.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;

use crate::app::App;
use crate::env::{BackendMode, Env};
use crate::http::{Reply, Request};

/// An `App` whose every writable path is in a fresh temporary directory.
/// Keep the `TempDir` alive for as long as the app is used.
pub fn app() -> (tempfile::TempDir, Arc<App>) {
    app_with(BackendMode::Emulator, |_| {})
}

/// Like [`app`], with a chance to prepare files (config, profiles) before
/// the services read them.
pub fn app_with(backend: BackendMode, prepare: impl FnOnce(&Env)) -> (tempfile::TempDir, Arc<App>) {
    let dir = tempfile::tempdir().expect("temporary directory");
    let env = Env::for_tests(dir.path(), backend);
    prepare(&env);
    let log = Arc::new(crate::logging::LogHooks::default());
    let app = App::new(env, log, crate::sentry_reporting::Reporting::init(false));
    (dir, app)
}

/// Send one request through the real route table.
pub async fn call(
    app: &Arc<App>,
    method: http::Method,
    uri: &str,
    body: impl Into<Bytes>,
) -> Reply {
    call_with_headers(app, method, uri, body, &[]).await
}

pub async fn call_with_headers(
    app: &Arc<App>,
    method: http::Method,
    uri: &str,
    body: impl Into<Bytes>,
    headers: &[(&str, &str)],
) -> Reply {
    let mut req = Request::synthetic(method, uri, body);
    for (name, value) in headers {
        if let (Ok(name), Ok(value)) = (
            http::header::HeaderName::from_bytes(name.as_bytes()),
            http::HeaderValue::from_str(value),
        ) {
            req.headers.insert(name, value);
        }
    }
    let router = crate::api::router(app);
    router.dispatch(app.clone(), req).await
}

/// The body parsed as JSON.
pub fn json(reply: &Reply) -> Value {
    serde_json::from_slice(reply.body()).expect("JSON body")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tests/test_api_base_handler.py: BaseHandler headers, OPTIONS → 204
    /// without a content type, tornado's 404 and 405 pages.
    #[tokio::test]
    async fn base_handler_behaviour() {
        let (_dir, app) = app();
        let reply = call(&app, http::Method::GET, "/api/v1/notifications", "").await;
        assert_eq!(reply.status_code(), 200);
        assert_eq!(reply.header("Access-Control-Allow-Origin"), Some("*"));
        assert_eq!(reply.header("Content-Type"), Some("application/json"));
        assert_eq!(reply.header("Cache-Control"), Some("no-cache"));

        let options = call(&app, http::Method::OPTIONS, "/api/v1/notifications", "").await;
        assert_eq!(options.status_code(), 204);
        assert_eq!(options.header("Content-Type"), None);
        assert_eq!(options.header("Access-Control-Allow-Origin"), Some("*"));

        let missing = call(&app, http::Method::GET, "/api/v1/missing", "").await;
        assert_eq!(missing.status_code(), 404);
        assert_eq!(missing.header("Access-Control-Allow-Origin"), None);
        assert_eq!(
            std::str::from_utf8(missing.body()).expect("utf8"),
            "<html><title>404: Not Found</title><body>404: Not Found</body></html>"
        );

        let wrong = call(&app, http::Method::PUT, "/api/v1/notifications", "").await;
        assert_eq!(wrong.status_code(), 405);
        assert_eq!(wrong.header("Content-Type"), Some("application/json"));
    }

    /// tests/test_local_access_policy.py: only X-Real-IP decides.
    #[tokio::test]
    async fn local_access_policy() {
        let (_dir, app) = app();
        let uri = "/api/v1/machine/root-password";
        let direct = call(&app, http::Method::GET, uri, "").await;
        assert_eq!(direct.status_code(), 200);
        assert_eq!(json(&direct)["root_password"], "root");
        for (ip, status) in [
            ("127.0.0.1", 200),
            ("::1", 200),
            ("::ffff:127.0.0.1", 200),
            ("192.168.1.5", 403),
            ("bogus", 403),
        ] {
            let reply =
                call_with_headers(&app, http::Method::GET, uri, "", &[("X-Real-IP", ip)]).await;
            assert_eq!(reply.status_code(), status, "{ip}");
        }
    }
}
