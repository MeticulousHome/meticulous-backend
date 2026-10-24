//! Port of `tests/test_local_access_policy.py`: which API routes refuse LAN
//! clients.
//!
//! `LOCAL_ONLY` lists every route and method that answers 403 to a request
//! nginx forwarded from another address (X-Real-IP), whatever Host it names;
//! everything else serves LAN clients. As in the Python, every route of
//! `API.get_routes()` is mounted at a literal `/probe/<index>` path with its
//! `prepare()` kept and each method it serves replaced by a stub, so only the
//! access check runs: nothing reaches the machine, the database or the disk.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::json;

use crate::app::App;
use crate::http::{Kind, Method, Reply, Request, Route, Router};
use crate::testing;

const LAN_CLIENT: &[(&str, &str)] = &[("Host", "meticulous.local"), ("X-Real-IP", "192.168.1.42")];
/// nginx forwards the Host the client sends, so a LAN client can name the
/// machine localhost.
const LAN_CLIENT_NAMING_LOCALHOST: &[(&str, &str)] =
    &[("Host", "localhost"), ("X-Real-IP", "192.168.1.42")];

const LOCAL_ONLY: [&str; 5] = [
    // The Dial writes Pour Over history; LAN clients only read and sync it.
    "POST /api/v1/history/pour-over",
    "GET /api/v1/history/upload-index",
    "GET /api/v1/machine/factory_reset",
    "GET /api/v1/machine/root-password",
    "GET /api/v1/smoke-validation",
];

/// `served_methods`: the verbs among get, post, put, patch, delete and
/// options the handler class overrides. `BaseHandler` defines `options`;
/// tornado's static and redirect handlers define `get`.
fn served_methods(route: &Route) -> Vec<Method> {
    let mut verbs = route.contract_methods();
    if route.kind == Kind::Base {
        verbs.push("options".into());
    }
    verbs
        .iter()
        .filter_map(|verb| match verb.as_str() {
            "get" => Some(Method::Get),
            "post" => Some(Method::Post),
            "put" => Some(Method::Put),
            "patch" => Some(Method::Patch),
            "delete" => Some(Method::Delete),
            "options" => Some(Method::Options),
            _ => None,
        })
        .collect()
}

async fn reached(_app: Arc<App>, _req: Request) -> Reply {
    Reply::base().json(json!({"reached": true}))
}

/// `probe(handler)`: the route at `/probe/<index>`, its access check kept,
/// every served method a stub. Static and redirect routes become plain
/// handlers so the stub answers their `get`.
fn probe(index: usize, route: &Route) -> Route {
    let kind = match route.kind {
        Kind::Base => Kind::Base,
        _ => Kind::Plain,
    };
    let mut probe = Route::new(
        &format!("/probe/{index}"),
        kind,
        route.module,
        route.handler,
    );
    probe.prepare = route.prepare;
    for method in served_methods(route) {
        probe = probe.on(method, reached);
    }
    probe
}

struct Policy {
    _dir: tempfile::TempDir,
    app: Arc<App>,
    /// (pattern, served methods) per probe index.
    routes: Vec<(String, Vec<Method>)>,
    router: Router,
}

fn policy() -> Policy {
    let (dir, app) = testing::app();
    let api = crate::api::api_routes(&app);
    let routes = api
        .iter()
        .map(|r| (r.pattern.clone(), served_methods(r)))
        .collect();
    let router = Router::new(
        api.iter()
            .enumerate()
            .map(|(index, route)| probe(index, route))
            .collect(),
    );
    Policy {
        _dir: dir,
        app,
        routes,
        router,
    }
}

impl Policy {
    async fn lan_refusals(&self, headers: &[(&str, &str)]) -> BTreeSet<String> {
        let mut refused = BTreeSet::new();
        for (index, (path, methods)) in self.routes.iter().enumerate() {
            for method in methods {
                let http_method =
                    http::Method::from_bytes(method.as_str().as_bytes()).expect("method");
                let mut req = Request::synthetic(http_method, &format!("/probe/{index}"), "");
                for (name, value) in headers {
                    req.headers.insert(
                        http::header::HeaderName::from_bytes(name.as_bytes()).expect("name"),
                        http::HeaderValue::from_str(value).expect("value"),
                    );
                }
                let reply = self.router.dispatch(self.app.clone(), req).await;
                if reply.status_code() == 403 {
                    refused.insert(format!("{} {path}", method.as_str()));
                } else {
                    assert_eq!(
                        testing::json(&reply),
                        json!({"reached": true}),
                        "{} {path}",
                        method.as_str()
                    );
                }
            }
        }
        refused
    }
}

/// tests/test_local_access_policy.py::TestLocalAccessPolicy::test_only_the_listed_methods_refuse_lan_clients
#[tokio::test]
async fn only_the_listed_methods_refuse_lan_clients() {
    let p = policy();

    let refused: BTreeSet<String> = p
        .lan_refusals(LAN_CLIENT)
        .await
        .into_iter()
        .filter(|route| !route.starts_with("OPTIONS"))
        .collect();

    let local_only: BTreeSet<String> = LOCAL_ONLY.iter().map(|r| r.to_string()).collect();
    assert_eq!(
        refused.difference(&local_only).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "refused to LAN clients, not in LOCAL_ONLY"
    );
    assert_eq!(
        local_only.difference(&refused).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "in LOCAL_ONLY, served to LAN clients"
    );
}

/// tests/test_local_access_policy.py::TestLocalAccessPolicy::test_naming_the_machine_localhost_does_not_make_a_lan_client_local
#[tokio::test]
async fn naming_the_machine_localhost_does_not_make_a_lan_client_local() {
    let p = policy();

    assert_eq!(
        p.lan_refusals(LAN_CLIENT_NAMING_LOCALHOST).await,
        p.lan_refusals(LAN_CLIENT).await
    );
}

/// tests/test_local_access_policy.py::TestLocalAccessPolicy::test_a_route_lan_clients_may_call_answers_their_preflight
#[tokio::test]
async fn a_route_lan_clients_may_call_answers_their_preflight() {
    let p = policy();
    let refused = p.lan_refusals(LAN_CLIENT).await;

    let missing: Vec<String> = p
        .routes
        .iter()
        .filter(|(path, methods)| {
            methods.contains(&Method::Options)
                && refused.contains(&format!("OPTIONS {path}"))
                && methods
                    .iter()
                    .filter(|m| **m != Method::Options)
                    .any(|m| !refused.contains(&format!("{} {path}", m.as_str())))
        })
        .map(|(path, _)| format!("OPTIONS {path}"))
        .collect();
    assert_eq!(missing, Vec::<String>::new());
}

/// Not a Python case: pins tornado's order on the real routes, which the
/// sweep above leaves open. `RequestHandler._execute` runs `prepare()` before
/// looking up the method, so `LocalAccessHandler` answers a LAN client's
/// preflight and a method the class lacks with its 403, not BaseHandler's
/// 204 or the 405; `PourOverHistoryHandler.prepare` exempts GET and OPTIONS.
#[tokio::test]
async fn local_access_check_runs_before_the_method_is_looked_up() {
    let (_dir, app) = testing::app();
    let lan = [("X-Real-IP", "192.168.1.42")];
    let forbidden =
        json!({"status": "error", "error": "This endpoint can only be accessed locally"});
    for (method, uri, lan_status, local_status) in [
        (
            http::Method::OPTIONS,
            "/api/v1/machine/root-password",
            403,
            204,
        ),
        (http::Method::PUT, "/api/v1/machine/root-password", 403, 405),
        (http::Method::OPTIONS, "/api/v1/history/pour-over", 204, 204),
        (http::Method::DELETE, "/api/v1/history/pour-over", 403, 405),
    ] {
        let remote = testing::call_with_headers(&app, method.clone(), uri, "", &lan).await;
        assert_eq!(remote.status_code(), lan_status, "LAN {method} {uri}");
        if lan_status == 403 {
            assert_eq!(testing::json(&remote), forbidden, "LAN {method} {uri}");
            assert_eq!(remote.header("Access-Control-Allow-Origin"), Some("*"));
        }
        let local = testing::call(&app, method.clone(), uri, "").await;
        assert_eq!(local.status_code(), local_status, "local {method} {uri}");
    }
}
