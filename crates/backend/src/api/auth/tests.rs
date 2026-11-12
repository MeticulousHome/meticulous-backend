//! Device authorization through the real route table: the `/api/v1/auth/*`
//! endpoints with their success and error answers, the guard in front of
//! every other API route (its skips and its 401), the local-only change of
//! `require_authorization`, and approval questions only the machine may
//! answer. Expected values come from the published device authorization
//! contract (sections 1, 2 and 4 to 7) and the constants in
//! `crate::device_auth`; a LAN client is one nginx forwarded (`X-Real-IP`).

use std::sync::Arc;

use http::Method;
use serde_json::{Value, json};

use crate::app::App;
use crate::config::keys;
use crate::device_auth::tests::network;
use crate::device_auth::{CODE_ALPHABET, CODE_LIFETIME_S, CodeSource, MAX_REQUESTS};
use crate::http::Reply;
use crate::notifications::{Notification, responses};
use crate::testing::{self, bearer, call, call_with_headers, json as body};
use crate::wifi::types::AP_CONNECTION;

const LAN_IP: &str = "192.168.1.42";

async fn lan(app: &Arc<App>, method: Method, uri: &str, payload: &str) -> Reply {
    call_with_headers(
        app,
        method,
        uri,
        payload.to_string(),
        &[("X-Real-IP", LAN_IP)],
    )
    .await
}

async fn lan_with(app: &Arc<App>, method: Method, uri: &str, payload: &str, auth: &str) -> Reply {
    call_with_headers(
        app,
        method,
        uri,
        payload.to_string(),
        &[("X-Real-IP", LAN_IP), ("Authorization", auth)],
    )
    .await
}

fn error(code: &str, message: &str) -> Value {
    json!({"status": "error", "error": message, "data": {"code": code}})
}

fn unauthorized() -> Value {
    error("UNAUTHORIZED", "This device is not authorized")
}

fn authorization_off(app: &App) {
    app.config
        .set(keys::USER, keys::REQUIRE_AUTHORIZATION, json!(false));
}

/// A code minted on the Dial's "Pair a device" screen.
async fn new_code(app: &Arc<App>) -> String {
    let reply = call(app, Method::POST, "/api/v1/auth/pairing?mode=new", "").await;
    assert_eq!(reply.status_code(), 200);
    body(&reply)["code"].as_str().expect("code").to_string()
}

async fn exchange(app: &Arc<App>, payload: Value) -> Reply {
    lan(
        app,
        Method::POST,
        "/api/v1/auth/token",
        &payload.to_string(),
    )
    .await
}

// ---- POST /auth/pairing ----

/// The Dial's "Pair a device" gets a code, the pair URL a QR code carries
/// (first IPv4 address of the current connection, the code in the
/// fragment) and its lifetime; the same request from the LAN is refused
/// with LOCAL_ONLY whatever the setting (contract section 5).
#[tokio::test]
async fn new_codes_come_from_the_machine_only() {
    let (_dir, app) = testing::app();
    app.auth
        .set_network(network("HomeNet", &["192.168.1.20/24", "fe80::1/64"]));
    let reply = call(&app, Method::POST, "/api/v1/auth/pairing?mode=new", "").await;
    assert_eq!(reply.status_code(), 200);
    assert_eq!(reply.header("Access-Control-Allow-Origin"), Some("*"));
    let answer = body(&reply);
    let code = answer["code"].as_str().expect("code");
    assert_eq!(code.len(), 8);
    assert!(code.bytes().all(|b| CODE_ALPHABET.contains(&b)));
    assert_eq!(
        answer,
        json!({
            "code": code,
            "pair_url": format!("http://192.168.1.20/#pair={code}"),
            "expires_in": CODE_LIFETIME_S,
        })
    );

    let token = bearer(&app);
    for setting in [true, false] {
        app.config
            .set(keys::USER, keys::REQUIRE_AUTHORIZATION, json!(setting));
        let refused = lan_with(
            &app,
            Method::POST,
            "/api/v1/auth/pairing?mode=new",
            "",
            &token,
        )
        .await;
        assert_eq!(refused.status_code(), 403, "required={setting}");
        assert_eq!(
            body(&refused),
            error("LOCAL_ONLY", "This endpoint can only be accessed locally")
        );
    }
}

/// Without an IPv4 address (no connection) the pair URL names the machine
/// by its mDNS name, `<hostname>.local` (contract section 4).
#[tokio::test]
async fn pair_url_falls_back_to_the_mdns_name() {
    let (_dir, app) = testing::app();
    app.auth.set_network(network("HomeNet", &[]));
    let reply = call(&app, Method::POST, "/api/v1/auth/pairing?mode=new", "").await;
    let answer = body(&reply);
    assert_eq!(
        answer["pair_url"],
        json!(format!(
            "http://meticulous-test.local/#pair={}",
            answer["code"].as_str().expect("code")
        ))
    );
}

/// "Allow another device" needs a valid token from a LAN client even while
/// authorization is off (contract section 5): otherwise anyone on the
/// network could mint codes once it is turned on.
#[tokio::test]
async fn grant_codes_need_a_token() {
    let (_dir, app) = testing::app();
    authorization_off(&app);
    let uri = "/api/v1/auth/pairing?mode=grant";
    let refused = lan(&app, Method::POST, uri, "").await;
    assert_eq!(refused.status_code(), 401);
    assert_eq!(body(&refused), unauthorized());
    let forged = lan_with(&app, Method::POST, uri, "", "Bearer a.b.c").await;
    assert_eq!(forged.status_code(), 401);

    let granted = lan_with(&app, Method::POST, uri, "", &bearer(&app)).await;
    assert_eq!(granted.status_code(), 200);
    let code = body(&granted)["code"].as_str().expect("code").to_string();
    assert_eq!(
        app.auth.take_code(&code).map(|c| c.source),
        Some(CodeSource::Grant)
    );
    assert_eq!(call(&app, Method::POST, uri, "").await.status_code(), 200);
}

/// A missing or unknown mode is INVALID_MODE (contract section 5).
#[tokio::test]
async fn pairing_needs_a_known_mode() {
    let (_dir, app) = testing::app();
    for uri in [
        "/api/v1/auth/pairing",
        "/api/v1/auth/pairing?mode=improv",
        "/api/v1/auth/pairing?mode=",
    ] {
        let reply = call(&app, Method::POST, uri, "").await;
        assert_eq!(reply.status_code(), 400, "{uri}");
        assert_eq!(
            body(&reply),
            error("INVALID_MODE", "mode must be new or grant"),
            "{uri}"
        );
    }
}

// ---- POST /auth/token ----

/// A LAN client without a token exchanges a code typed as the Dial shows it
/// (`ABCD-EFGH`, any case) for a token naming it, which then gets it into
/// the API; the Dial is told, which closes its pairing screen (contract
/// sections 4 and 5).
#[tokio::test]
async fn a_code_is_exchanged_for_a_token_once() {
    let (_dir, app) = testing::app();
    let code = new_code(&app).await;
    let typed = format!("{}-{}", &code[..4], code[4..].to_lowercase());

    let reply = exchange(&app, json!({"code": typed, "name": "Angel's iPhone"})).await;
    assert_eq!(reply.status_code(), 200);
    assert_eq!(reply.header("Access-Control-Allow-Origin"), Some("*"));
    let answer = body(&reply);
    assert_eq!(answer["device"]["name"], json!("Angel's iPhone"));
    let token = answer["token"].as_str().expect("token");
    let claims = app.auth.verify(token).expect("valid token");
    assert_eq!(answer["device"]["id"], json!(claims.sub));
    assert!(uuid::Uuid::parse_str(&claims.sub).is_ok());

    let shown = app.notifications.unacknowledged();
    assert_eq!(shown.len(), 1);
    assert_eq!(
        shown[0].message,
        "\u{201c}Angel's iPhone\u{201d} can now use this machine"
    );
    assert_eq!(shown[0].responses, [responses::OK]);

    let auth = format!("Bearer {token}");
    let notifications = lan_with(&app, Method::GET, "/api/v1/notifications", "", &auth).await;
    assert_eq!(notifications.status_code(), 200);

    let again = exchange(&app, json!({"code": code})).await;
    assert_eq!(again.status_code(), 401);
    assert_eq!(
        body(&again),
        error("INVALID_CODE", "This code is not valid")
    );
}

/// The token's name: the device's own (sanitised), else the name the code
/// was bound to (the BLE name, the requested name), else "Unnamed device";
/// the Improv code's exchange tells the Dial nothing, the user approved it
/// there a moment ago (contract section 5).
#[tokio::test]
async fn the_token_name_falls_back_to_the_code_and_then_a_default() {
    let (_dir, app) = testing::app();
    let bound = app
        .auth
        .mint_code(CodeSource::Improv, Some("Ana's Pixel".into()));
    let reply = exchange(&app, json!({"code": bound, "name": " \u{7}<> "})).await;
    assert_eq!(body(&reply)["device"]["name"], json!("Ana's Pixel"));
    assert!(app.notifications.unacknowledged().is_empty());

    let unbound = app.auth.mint_code(CodeSource::Grant, None);
    let reply = exchange(&app, json!({"code": unbound, "name": 42})).await;
    assert_eq!(body(&reply)["device"]["name"], json!("Unnamed device"));

    let long = new_code(&app).await;
    let reply = exchange(
        &app,
        json!({"code": long, "name": "<b>".repeat(5) + &"x".repeat(60)}),
    )
    .await;
    let name = body(&reply)["device"]["name"]
        .as_str()
        .expect("name")
        .to_string();
    assert_eq!(name.chars().count(), 48);
    assert!(name.starts_with("bbbbbxx"));
    assert!(name.ends_with('\u{2026}'));
}

/// No code in the body is INVALID_REQUEST, not a failed attempt.
#[tokio::test]
async fn exchanging_needs_a_code() {
    let (_dir, app) = testing::app();
    for payload in [
        "",
        "{}",
        "[]",
        "not json",
        r#"{"code": 12345678}"#,
        r#"{"code": "  "}"#,
    ] {
        let reply = lan(&app, Method::POST, "/api/v1/auth/token", payload).await;
        assert_eq!(reply.status_code(), 400, "{payload}");
        assert_eq!(
            body(&reply)["data"]["code"],
            json!("INVALID_REQUEST"),
            "{payload}"
        );
    }
}

/// Guessing codes is throttled like `POST /machine/unlock` (UnlockThrottle:
/// more than 10 failures in 60 s lock for 30 s): 429 TOO_MANY_ATTEMPTS
/// with Retry-After, even for a valid code (contract section 5).
#[tokio::test]
async fn guessing_codes_is_throttled() {
    let (_dir, app) = testing::app();
    let code = new_code(&app).await;
    for attempt in 0..=crate::limited_access::UNLOCK_MAX_FAILURES {
        let reply = exchange(&app, json!({"code": "ZZZZZZZZ"})).await;
        assert_eq!(reply.status_code(), 401, "attempt {attempt}");
    }
    let locked = exchange(&app, json!({"code": code})).await;
    assert_eq!(locked.status_code(), 429);
    assert_eq!(
        body(&locked),
        error("TOO_MANY_ATTEMPTS", "Too many attempts, try again later")
    );
    let retry_after: u64 = locked
        .header("Retry-After")
        .expect("Retry-After")
        .parse()
        .expect("seconds");
    assert!((1..=30).contains(&retry_after), "{retry_after}");
}

// ---- GET /auth/validate ----

/// 200 exactly when the request would get into the API, with what made it
/// so; 401 UNAUTHORIZED with `required` otherwise (contract section 5).
#[tokio::test]
async fn validate_says_whether_the_api_would_answer() {
    let (_dir, app) = testing::app();
    let uri = "/api/v1/auth/validate";

    let local = call(&app, Method::GET, uri, "").await;
    assert_eq!(local.status_code(), 200);
    assert_eq!(
        body(&local),
        json!({"required": true, "local": true, "token_valid": false, "device": null})
    );

    let refused = lan(&app, Method::GET, uri, "").await;
    assert_eq!(refused.status_code(), 401);
    assert_eq!(refused.header("WWW-Authenticate"), Some("Bearer"));
    let mut expected = unauthorized();
    expected["required"] = json!(true);
    assert_eq!(body(&refused), expected);

    let code = new_code(&app).await;
    let paired = body(&exchange(&app, json!({"code": code, "name": "Phone"})).await);
    let auth = format!("Bearer {}", paired["token"].as_str().expect("token"));
    let valid = lan_with(&app, Method::GET, uri, "", &auth).await;
    assert_eq!(valid.status_code(), 200);
    assert_eq!(
        body(&valid),
        json!({
            "required": true,
            "local": false,
            "token_valid": true,
            "device": paired["device"],
        })
    );

    let forged = lan_with(&app, Method::GET, uri, "", "Bearer x.y.z").await;
    assert_eq!(forged.status_code(), 401);

    authorization_off(&app);
    let open = lan(&app, Method::GET, uri, "").await;
    assert_eq!(open.status_code(), 200);
    assert_eq!(
        body(&open),
        json!({"required": false, "local": false, "token_valid": false, "device": null})
    );
}

// ---- POST /auth/request, GET /auth/request/<id> ----

async fn request_access(app: &Arc<App>, name: &str) -> Reply {
    lan(
        app,
        Method::POST,
        "/api/v1/auth/request",
        &json!({"name": name}).to_string(),
    )
    .await
}

async fn poll(app: &Arc<App>, id: &str) -> Reply {
    lan(app, Method::GET, &format!("/api/v1/auth/request/{id}"), "").await
}

/// The Dial's question for the pending request.
fn question(app: &App) -> Notification {
    let pending = app.notifications.unacknowledged();
    assert_eq!(pending.len(), 1, "one question on the Dial");
    pending[0].clone()
}

async fn answer_locally(app: &Arc<App>, id: &str, response: &str) {
    let reply = call(
        app,
        Method::POST,
        "/api/v1/notifications/acknowledge",
        json!({"id": id, "response": response}).to_string(),
    )
    .await;
    assert_eq!(reply.status_code(), 200);
}

/// "Request access": a LAN client without a token asks, the Dial shows
/// `“<name>” is asking to use this machine` with Deny first, Allow on the
/// machine approves, the poll returns a code bound to the requested name,
/// and the code buys a token with that name (contract sections 5 and 6).
#[tokio::test]
async fn an_allowed_request_ends_with_a_token() {
    let (_dir, app) = testing::app();
    let reply = request_access(&app, "Chrome on macOS").await;
    assert_eq!(reply.status_code(), 202);
    let answer = body(&reply);
    assert_eq!(answer["expires_in"], json!(60));
    let id = answer["id"].as_str().expect("id").to_string();
    assert!(uuid::Uuid::parse_str(&id).is_ok());

    let asked = question(&app);
    assert_eq!(
        asked.message,
        "\u{201c}Chrome on macOS\u{201d} is asking to use this machine"
    );
    assert_eq!(asked.responses, [responses::DENY, responses::ALLOW]);
    assert!(asked.local_answer);
    assert_eq!(body(&poll(&app, &id).await), json!({"status": "pending"}));

    answer_locally(&app, &asked.id, responses::ALLOW).await;

    let approved = body(&poll(&app, &id).await);
    assert_eq!(approved["status"], json!("approved"));
    assert_eq!(approved["expires_in"], json!(CODE_LIFETIME_S));
    let paired = body(&exchange(&app, json!({"code": approved["code"]})).await);
    assert_eq!(paired["device"]["name"], json!("Chrome on macOS"));
}

/// Deny on the Dial is final: the requester reads `denied`, no code.
#[tokio::test]
async fn a_denied_request_gets_no_code() {
    let (_dir, app) = testing::app();
    let id = body(&request_access(&app, "Laptop").await)["id"]
        .as_str()
        .expect("id")
        .to_string();
    answer_locally(&app, &question(&app).id, responses::DENY).await;
    assert_eq!(body(&poll(&app, &id).await), json!({"status": "denied"}));
}

/// Nobody answers within 60 s: `expired`, and the Dial's question becomes
/// a notice under the same id (the Dial only drops a notification once it
/// is answered or replaced). Records are forgotten about 10 minutes later
/// (contract section 5).
#[tokio::test]
async fn an_unanswered_request_expires() {
    let (_dir, app) = testing::app();
    let id = body(&request_access(&app, "Laptop").await)["id"]
        .as_str()
        .expect("id")
        .to_string();
    let asked = question(&app);
    app.auth.advance(60);

    assert_eq!(body(&poll(&app, &id).await), json!({"status": "expired"}));
    let notice = question(&app);
    assert_eq!(notice.id, asked.id);
    assert_eq!(notice.responses, [responses::OK]);
    assert_eq!(
        notice.message,
        "\u{201c}Laptop\u{201d} was not allowed to use this machine: nobody answered in time"
    );

    app.auth.advance(601);
    let gone = poll(&app, &id).await;
    assert_eq!(gone.status_code(), 404);
    assert_eq!(body(&gone), error("NOT_FOUND", "No such request"));
}

/// One question at a time (409 REQUEST_PENDING), and at most MAX_REQUESTS
/// (5) per minute (429 TOO_MANY_ATTEMPTS with Retry-After): each request
/// rings the machine (contract section 5).
#[tokio::test]
async fn requests_are_one_at_a_time_and_rate_limited() {
    let (_dir, app) = testing::app();
    request_access(&app, "First").await;
    let busy = request_access(&app, "Second").await;
    assert_eq!(busy.status_code(), 409);
    assert_eq!(
        body(&busy),
        error("REQUEST_PENDING", "Another device is waiting for an answer")
    );
    answer_locally(&app, &question(&app).id, responses::DENY).await;
    for _ in 1..MAX_REQUESTS {
        assert_eq!(request_access(&app, "Again").await.status_code(), 202);
        answer_locally(&app, &question(&app).id, responses::DENY).await;
    }
    let throttled = request_access(&app, "One too many").await;
    assert_eq!(throttled.status_code(), 429);
    assert_eq!(body(&throttled)["data"]["code"], json!("TOO_MANY_ATTEMPTS"));
    assert!(throttled.header("Retry-After").is_some());

    app.auth.advance(60);
    assert_eq!(request_access(&app, "Later").await.status_code(), 202);
}

/// A request needs a printable name; an unknown id is NOT_FOUND as JSON
/// (a 404 page would read as "no device authorization here").
#[tokio::test]
async fn request_errors() {
    let (_dir, app) = testing::app();
    for payload in ["", "{}", r#"{"name": " <&> "}"#, r#"{"name": 7}"#] {
        let reply = lan(&app, Method::POST, "/api/v1/auth/request", payload).await;
        assert_eq!(reply.status_code(), 400, "{payload}");
        assert_eq!(body(&reply)["data"]["code"], json!("INVALID_REQUEST"));
    }
    for id in ["unknown", "00000000-0000-4000-8000-000000000000"] {
        let reply = poll(&app, id).await;
        assert_eq!(reply.status_code(), 404, "{id}");
        assert_eq!(body(&reply), error("NOT_FOUND", "No such request"));
    }
}

// ---- POST /auth/sign_out_all ----

/// "Sign out all devices" is the Dial's (403 to the LAN, before the token
/// check); afterwards no earlier token gets in and pending requests are
/// refused (contract sections 3 and 5).
#[tokio::test]
async fn sign_out_all_logs_every_device_out() {
    let (_dir, app) = testing::app();
    let token = bearer(&app);
    let uri = "/api/v1/auth/sign_out_all";
    let refused = lan_with(&app, Method::POST, uri, "", &token).await;
    assert_eq!(refused.status_code(), 403);
    let id = body(&request_access(&app, "Laptop").await)["id"]
        .as_str()
        .expect("id")
        .to_string();

    let reply = call(&app, Method::POST, uri, "").await;
    assert_eq!(reply.status_code(), 200);
    assert_eq!(body(&reply), json!({"status": "ok"}));

    let validate = lan_with(&app, Method::GET, "/api/v1/auth/validate", "", &token).await;
    assert_eq!(validate.status_code(), 401);
    assert_eq!(body(&poll(&app, &id).await), json!({"status": "denied"}));
    assert!(app.notifications.unacknowledged().is_empty());
}

// ---- the guard ----

/// A LAN client without a token gets 401 UNAUTHORIZED on any API route,
/// with `WWW-Authenticate: Bearer` and the CORS headers a browser on
/// another origin needs to read it (contract section 7).
#[tokio::test]
async fn the_guard_refuses_lan_clients_without_a_token() {
    let (_dir, app) = testing::app();
    for (method, uri) in [
        (Method::GET, "/api/v1/notifications"),
        (Method::POST, "/api/v1/settings"),
        (Method::GET, "/api/v1/history/pour-over"),
        (Method::GET, "/api/v1/profile/list"),
    ] {
        let reply = lan(&app, method.clone(), uri, "{}").await;
        assert_eq!(reply.status_code(), 401, "{method} {uri}");
        assert_eq!(body(&reply), unauthorized(), "{method} {uri}");
        assert_eq!(reply.header("WWW-Authenticate"), Some("Bearer"));
        assert_eq!(reply.header("Access-Control-Allow-Origin"), Some("*"));
        let expired = lan_with(&app, method.clone(), uri, "{}", "Bearer x.y.z").await;
        assert_eq!(expired.status_code(), 401, "{method} {uri}");
    }
    // A route that refuses the LAN outright still says so first.
    let local_only = lan(&app, Method::GET, "/api/v1/machine/root-password", "").await;
    assert_eq!(local_only.status_code(), 403);
}

/// What the guard lets through without a token (contract sections 1, 2
/// and 7): preflights, the machine itself, the machine's hotspot, every
/// client while the setting is off, and the public routes (discovery,
/// profile images, the auth routes); and a LAN client with a valid token.
#[tokio::test]
async fn the_guard_skips() {
    let (_dir, app) = testing::app();
    let uri = "/api/v1/notifications";

    let preflight = lan(&app, Method::OPTIONS, uri, "").await;
    assert_eq!(preflight.status_code(), 204);
    assert_eq!(call(&app, Method::GET, uri, "").await.status_code(), 200);
    let loopback = call_with_headers(&app, Method::GET, uri, "", &[("X-Real-IP", "::1")]).await;
    assert_eq!(loopback.status_code(), 200);
    assert_eq!(
        lan_with(&app, Method::GET, uri, "", &bearer(&app))
            .await
            .status_code(),
        200
    );

    for (method, public) in [
        (Method::GET, "/api/v1/machine"),
        (Method::GET, "/api/v1/profile/image/missing.png"),
        (
            Method::GET,
            "/api/v1/pour-over/profile/image/00000000-0000-4000-8000-000000000000",
        ),
        (Method::GET, "/api/v1/auth/request/unknown"),
    ] {
        let reply = lan(&app, method, public, "").await;
        assert_ne!(reply.status_code(), 401, "{public}");
    }

    // The hotspot's own clients are next to the machine; a client of the
    // same address range on another network is not.
    app.auth
        .set_network(network(AP_CONNECTION, &["10.42.0.1/24"]));
    let hotspot =
        call_with_headers(&app, Method::GET, uri, "", &[("X-Real-IP", "10.42.0.23")]).await;
    assert_eq!(hotspot.status_code(), 200);
    app.auth.set_network(network("HomeNet", &["10.42.0.1/24"]));
    let home = call_with_headers(&app, Method::GET, uri, "", &[("X-Real-IP", "10.42.0.23")]).await;
    assert_eq!(home.status_code(), 401);

    authorization_off(&app);
    assert_eq!(lan(&app, Method::GET, uri, "").await.status_code(), 200);
}

// ---- require_authorization ----

/// The setting is switched on the machine: a LAN client (even a paired
/// one) changing it gets 403 LOCAL_ONLY and nothing changes, while sending
/// the stored value back passes; the Dial changes it (contract section 2).
#[tokio::test]
async fn only_the_machine_changes_require_authorization() {
    let (_dir, app) = testing::app();
    let token = bearer(&app);
    let off = json!({"require_authorization": false, "enable_sounds": false}).to_string();
    let refused = lan_with(&app, Method::POST, "/api/v1/settings", &off, &token).await;
    assert_eq!(refused.status_code(), 403);
    assert_eq!(
        body(&refused),
        error(
            "LOCAL_ONLY",
            "This setting can only be changed on the machine"
        )
    );
    assert!(app.auth.required());
    assert!(app.config.get_bool(keys::USER, keys::SOUNDS_ENABLED));

    let same = json!({"require_authorization": true}).to_string();
    let kept = lan_with(&app, Method::POST, "/api/v1/settings", &same, &token).await;
    assert_eq!(kept.status_code(), 200);

    let changed = call(&app, Method::POST, "/api/v1/settings", off).await;
    assert_eq!(changed.status_code(), 200);
    assert_eq!(body(&changed)["require_authorization"], json!(false));
    assert!(!app.auth.required());
    assert_eq!(
        lan(&app, Method::GET, "/api/v1/settings", "")
            .await
            .status_code(),
        200
    );
}

// ---- questions answered on the machine only ----

/// A question marked `answer_locally` (the Wi-Fi approval, an access
/// request) refuses a LAN client's answer with 403 LOCAL_ONLY and stays
/// open; the machine answers it; other notifications still take LAN
/// answers (contract section 6).
#[tokio::test]
async fn approval_questions_take_the_machines_answer_only() {
    let (_dir, app) = testing::app();
    let token = bearer(&app);
    let approval = Notification::with_responses("Let it in?", &[responses::DENY, responses::ALLOW])
        .answer_locally();
    let ordinary = Notification::new("Descale soon");
    let (approval_id, ordinary_id) = (approval.id.clone(), ordinary.id.clone());
    app.notifications.add(approval);
    app.notifications.add(ordinary);
    let uri = "/api/v1/notifications/acknowledge";

    let refused = lan_with(
        &app,
        Method::POST,
        uri,
        &json!({"id": approval_id, "response": "Allow"}).to_string(),
        &token,
    )
    .await;
    assert_eq!(refused.status_code(), 403);
    assert_eq!(
        body(&refused),
        error(
            "LOCAL_ONLY",
            "This notification can only be answered on the machine"
        )
    );
    assert_eq!(app.notifications.is_acknowledged(&approval_id), Some(false));

    let answered = lan_with(
        &app,
        Method::POST,
        uri,
        &json!({"id": ordinary_id, "response": "Ok"}).to_string(),
        &token,
    )
    .await;
    assert_eq!(answered.status_code(), 200);
    answer_locally(&app, &approval_id, "Allow").await;
    assert_eq!(app.notifications.is_acknowledged(&approval_id), Some(true));
}

/// The Socket.IO `notification` event follows the same rule: a remote
/// socket's answer to an approval question is ignored, the machine's is
/// taken (contract section 6).
#[tokio::test]
async fn approval_questions_ignore_remote_sockets() {
    let (_dir, app) = testing::app();
    let approval = Notification::with_responses("Let it in?", &[responses::DENY, responses::ALLOW])
        .answer_locally();
    let id = approval.id.clone();
    app.notifications.add(approval);
    let answer = Value::String(json!({"id": id, "response": "Allow"}).to_string());

    crate::events::on_notification(&app, Some(answer.clone()), false);
    assert_eq!(app.notifications.is_acknowledged(&id), Some(false));
    crate::events::on_notification(&app, Some(answer), true);
    assert_eq!(app.notifications.is_acknowledged(&id), Some(true));
}
