//! Port of `tests/test_api_base_handler.py`: which requests
//! `LocalAccessHandler` lets through. As in the Python, the check is
//! exercised on a handler of its own at `/local` whose `get` only says it
//! was reached.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::api::{is_loopback, local_only};
use crate::app::App;
use crate::http::{Kind, Reply, Request, Route, Router};
use crate::testing;

fn forbidden() -> Value {
    json!({"status": "error", "error": "This endpoint can only be accessed locally"})
}

/// `LocalOnly(LocalAccessHandler)` mounted at `/local`.
fn local_router() -> Router {
    Router::new(vec![
        Route::new("/local", Kind::Base, "tests", "LocalOnly")
            .prepare(local_only)
            .get(|_app: Arc<App>, _req: Request| async {
                Reply::base().json(json!({"reached": true}))
            }),
    ])
}

async fn fetch(app: &Arc<App>, headers: &[(&str, &str)]) -> Reply {
    let mut req = Request::synthetic(http::Method::GET, "/local", "");
    for (name, value) in headers {
        req.headers.insert(
            http::header::HeaderName::from_bytes(name.as_bytes()).expect("header name"),
            http::HeaderValue::from_str(value).expect("header value"),
        );
    }
    local_router().dispatch(app.clone(), req).await
}

async fn assert_forbidden(app: &Arc<App>, headers: &[(&str, &str)]) {
    let reply = fetch(app, headers).await;
    assert_eq!(reply.status_code(), 403, "{headers:?}");
    assert_eq!(testing::json(&reply), forbidden(), "{headers:?}");
}

/// tests/test_api_base_handler.py::TestLocalAccessHandler::test_ipv6_lan_client_addressing_the_machine_as_localhost_is_forbidden
#[tokio::test]
async fn ipv6_lan_client_addressing_the_machine_as_localhost_is_forbidden() {
    let (_dir, app) = testing::app();
    assert_forbidden(
        &app,
        &[
            ("X-Real-IP", "fe80::1c2b:3aff:fe4d:5e6f"),
            ("Host", "localhost"),
        ],
    )
    .await;
}

/// tests/test_api_base_handler.py::TestLocalAccessHandler::test_proxy_header_that_is_not_an_address_is_forbidden
#[tokio::test]
async fn proxy_header_that_is_not_an_address_is_forbidden() {
    let (_dir, app) = testing::app();
    // nginx always sends an address; anything else is refused rather than trusted.
    for value in ["localhost", "unknown", ""] {
        assert_forbidden(&app, &[("X-Real-IP", value), ("Host", "localhost")]).await;
    }
}

/// tests/test_api_base_handler.py::test_loopback_addresses
#[test]
fn loopback_addresses() {
    for address in ["127.0.0.1", "127.0.1.1", "::1", "::ffff:127.0.0.1"] {
        assert!(is_loopback(address), "{address}");
    }
}

/// tests/test_api_base_handler.py::test_non_loopback_addresses
#[test]
fn non_loopback_addresses() {
    for address in [
        "192.168.1.42",
        "10.0.0.7",
        "fe80::1",
        "::ffff:192.168.1.42",
        "localhost",
        "",
    ] {
        assert!(!is_loopback(address), "{address:?}");
    }
}
