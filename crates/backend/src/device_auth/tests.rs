//! Device authorization's building blocks: the HS256 tokens, the pairing
//! codes, the signing secret on disk, and who is let in (the hotspot
//! exemption and its single NetworkManager read at a time, Socket.IO
//! handshakes, the sweep after a sign-out and at a token's expiry). Expected
//! values come from the published contract (device authorization, sections
//! 1 to 4 and 7) and the constants in `crate::device_auth`.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};

use serde_json::json;

use super::*;
use crate::config::keys;
use crate::testing;
use crate::wifi::types::AP_CONNECTION;

const SECRET: &[u8] = b"0123456789abcdef0123456789abcdef";

fn claims(exp: Option<i64>) -> Claims {
    Claims {
        sub: "8f6d3c1e-2b4a-4c5d-9e8f-0a1b2c3d4e5f".into(),
        name: "Test phone".into(),
        iat: 1_700_000_000,
        exp,
    }
}

/// A token built by hand: `header` and `payload` as JSON text, signed with
/// `secret` over the usual signing input.
fn forge(header: &str, payload: &str, secret: &[u8]) -> String {
    let input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header),
        URL_SAFE_NO_PAD.encode(payload)
    );
    let mut mac = hmac(secret).expect("key");
    mac.update(input.as_bytes());
    format!(
        "{input}.{}",
        URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
    )
}

pub(crate) struct FakeNetwork(pub WifiSystemConfig);

impl NetworkState for FakeNetwork {
    fn current_config(&self) -> PyResult<WifiSystemConfig> {
        Ok(self.0.clone())
    }
}

/// The machine connected to `connection` with `ips`.
pub(crate) fn network(connection: &str, ips: &[&str]) -> Arc<FakeNetwork> {
    let mut config = WifiSystemConfig::disconnected("meticulous-test".into());
    config.connected = !ips.is_empty();
    config.connection_name = Some(connection.into());
    config.ips = ips.iter().map(|ip| IpNet::parse(ip).expect("ip")).collect();
    Arc::new(FakeNetwork(config))
}

/// A NetworkManager that counts its reads, each taking `delay`, and fails
/// them once `fail` is set.
struct CountingNetwork {
    config: WifiSystemConfig,
    delay: Duration,
    reads: AtomicUsize,
    fail: AtomicBool,
}

impl CountingNetwork {
    fn hotspot(delay: Duration) -> Arc<CountingNetwork> {
        Arc::new(CountingNetwork {
            config: network(AP_CONNECTION, &["10.42.0.1/24"]).0.clone(),
            delay,
            reads: AtomicUsize::new(0),
            fail: AtomicBool::new(false),
        })
    }

    fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }
}

impl NetworkState for CountingNetwork {
    fn current_config(&self) -> PyResult<WifiSystemConfig> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        if self.fail.load(Ordering::SeqCst) {
            return Err(crate::py::PyError::new(
                "CalledProcessError",
                "nmcli failed",
            ));
        }
        Ok(self.config.clone())
    }
}

fn lan(ip: &str) -> http::HeaderMap {
    let mut headers = http::HeaderMap::new();
    headers.insert("X-Real-IP", ip.parse().expect("header"));
    headers
}

// ---- tokens ----

/// A token the machine signed verifies to the same claims, exp or not; the
/// header is exactly `{"alg":"HS256","typ":"JWT"}` (contract section 3),
/// which other JWT libraries read.
#[test]
fn signed_token_verifies_to_its_claims() {
    for exp in [None, Some(1_700_000_600)] {
        let token = sign(SECRET, &claims(exp)).expect("signed");
        let header = token.split('.').next().expect("header");
        assert_eq!(
            URL_SAFE_NO_PAD.decode(header).expect("base64"),
            br#"{"alg":"HS256","typ":"JWT"}"#
        );
        assert_eq!(verify(SECRET, &token, 1_700_000_001), Some(claims(exp)));
    }
}

/// alg pinned to HS256 (contract section 3): `none` with no signature, or
/// another algorithm, never gets in, even with the right claims; a token
/// the attacker signed with `none` and an empty signature is the classic
/// JWT bypass.
#[test]
fn other_algorithms_are_refused() {
    let payload = r#"{"sub":"x","name":"n","iat":1}"#;
    let none = format!(
        "{}.{}.",
        URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#),
        URL_SAFE_NO_PAD.encode(payload)
    );
    assert_eq!(verify(SECRET, &none, 2), None);
    for header in [
        r#"{"alg":"none","typ":"JWT"}"#,
        r#"{"alg":"HS512","typ":"JWT"}"#,
        r#"{"alg":"hs256","typ":"JWT"}"#,
        r#"{"typ":"JWT"}"#,
    ] {
        assert_eq!(
            verify(SECRET, &forge(header, payload, SECRET), 2),
            None,
            "{header}"
        );
    }
    // The same claims under the pinned header pass, so the header decided.
    assert!(verify(SECRET, &forge(JWT_HEADER, payload, SECRET), 2).is_some());
}

/// A changed payload, a changed signature, another machine's secret, or
/// a token cut short all fail: the MAC covers header and payload.
#[test]
fn tampered_tokens_are_refused() {
    let token = sign(SECRET, &claims(None)).expect("signed");
    let parts: Vec<&str> = token.split('.').collect();
    let other_payload = URL_SAFE_NO_PAD.encode(r#"{"sub":"someone-else","name":"x","iat":1}"#);
    let mut signature = URL_SAFE_NO_PAD.decode(parts[2]).expect("signature");
    signature[0] ^= 0x01;
    for bad in [
        format!("{}.{other_payload}.{}", parts[0], parts[2]),
        format!(
            "{}.{}.{}",
            parts[0],
            parts[1],
            URL_SAFE_NO_PAD.encode(&signature)
        ),
        format!("{}.{}", parts[0], parts[1]),
        format!("{token}.extra"),
        "not a token".to_string(),
        String::new(),
    ] {
        assert_eq!(verify(SECRET, &bad, 2), None, "{bad}");
    }
    assert_eq!(verify(b"another machine's secret........", &token, 2), None);
}

/// `exp` is honoured when present (contract section 3): refused from that
/// second on; a token without `exp` never expires.
#[test]
fn expired_tokens_are_refused() {
    let token = sign(SECRET, &claims(Some(1_000))).expect("signed");
    assert!(verify(SECRET, &token, 999).is_some());
    assert_eq!(verify(SECRET, &token, 1_000), None);
    assert_eq!(verify(SECRET, &token, 5_000), None);
    let forever = sign(SECRET, &claims(None)).expect("signed");
    assert!(verify(SECRET, &forever, i64::MAX).is_some());
    // A token missing a claim the machine always writes is not one of its.
    let partial = forge(JWT_HEADER, r#"{"name":"n","iat":1}"#, SECRET);
    assert_eq!(verify(SECRET, &partial, 2), None);
}

/// Clients send `Authorization: Bearer <jwt>` (contract section 3); the
/// scheme is case-insensitive, anything else is no token.
#[test]
fn bearer_header_parsing() {
    for (value, expected) in [
        ("Bearer abc.def.ghi", Some("abc.def.ghi")),
        ("bearer   abc", Some("abc")),
        ("Basic abc", None),
        ("Bearer", None),
        ("Bearer ", None),
    ] {
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::AUTHORIZATION, value.parse().expect("value"));
        assert_eq!(bearer(&headers), expected, "{value:?}");
    }
    assert_eq!(bearer(&http::HeaderMap::new()), None);
}

/// `system.auth_token_lifetime` > 0 adds `exp`; the token then stops
/// working once that many seconds pass. The default 0 issues tokens that
/// never expire (contract section 3).
#[test]
fn token_lifetime_comes_from_the_system_setting() {
    let (_dir, app) = testing::app();
    let (token, claims) = app.auth.issue("Phone").expect("issued");
    assert_eq!(claims.exp, None);
    app.config
        .set(keys::SYSTEM, keys::AUTH_TOKEN_LIFETIME, json!(3600));
    let (expiring, claims) = app.auth.issue("Phone").expect("issued");
    assert_eq!(claims.exp, Some(claims.iat + 3600));
    assert!(app.auth.verify(&expiring).is_some());
    app.auth.advance(3601);
    assert_eq!(app.auth.verify(&expiring), None);
    assert!(app.auth.verify(&token).is_some());
}

// ---- the secret ----

/// The secret is 32 bytes in `<config_dir>/device_auth.key`, mode 0600,
/// made on first use; a restarted backend (a new `DeviceAuth` on the same
/// file) still accepts the tokens it issued (contract section 3).
#[test]
fn secret_file_is_private_and_survives_a_restart() {
    let (_dir, app) = testing::app();
    let path = app.env.paths.config_dir.join(KEY_FILE);
    assert!(!path.exists());
    let (token, _) = app.auth.issue("Phone").expect("issued");
    let metadata = std::fs::metadata(&path).expect("key file");
    assert_eq!(metadata.len(), 32);
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);

    let restarted = DeviceAuth::new(
        path.clone(),
        app.config.clone(),
        app.sio.clone(),
        app.notifications.clone(),
        Arc::new(NoNetwork),
    );
    assert!(restarted.verify(&token).is_some());

    // A damaged key is replaced, not trusted.
    std::fs::write(&path, b"short").expect("damage");
    let replaced = DeviceAuth::new(
        path.clone(),
        app.config.clone(),
        app.sio.clone(),
        app.notifications.clone(),
        Arc::new(NoNetwork),
    );
    assert_eq!(replaced.verify(&token), None);
    assert_eq!(std::fs::metadata(&path).expect("key").len(), 32);
}

/// "Sign out all devices" (contract section 3): a new secret, written
/// atomically and still 0600, so every token stops working; live codes go
/// too.
#[test]
fn sign_out_all_rotates_the_secret_and_drops_codes() {
    let (_dir, app) = testing::app();
    let path = app.env.paths.config_dir.join(KEY_FILE);
    let (token, _) = app.auth.issue("Phone").expect("issued");
    let before = std::fs::read(&path).expect("key");
    let code = app.auth.mint_code(CodeSource::New, None);

    app.auth.sign_out_all().expect("signed out");

    assert_eq!(app.auth.verify(&token), None);
    let after = std::fs::read(&path).expect("key");
    assert_eq!(after.len(), 32);
    assert_ne!(after, before);
    assert_eq!(
        std::fs::metadata(&path).expect("key").permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(app.auth.take_code(&code), None);
    let (fresh, _) = app.auth.issue("Phone").expect("issued");
    assert!(app.auth.verify(&fresh).is_some());
}

// ---- pairing codes ----

/// Typed codes are read the way the contract (section 4) says: case,
/// spaces and the display dash do not matter, I and L read as 1, O as 0;
/// U and other characters outside Crockford base32 make it no code.
#[test]
fn codes_are_normalised() {
    for (typed, expected) in [
        ("ABCD-EFGH", Some("ABCDEFGH")),
        ("abcd efgh", Some("ABCDEFGH")),
        (" a1b2-c3d4 ", Some("A1B2C3D4")),
        ("OIL0-ABCD", Some("0110ABCD")),
        ("ABCD-EFGU", None),
        ("ABCD-EFG", None),
        ("ABCD-EFGHJ", None),
        ("ABCD_EFGH", None),
        ("ÄBCD-EFGH", None),
    ] {
        assert_eq!(normalize_code(typed).as_deref(), expected, "{typed:?}");
    }
}

/// A minted code is 8 Crockford base32 characters, exchanges once (with
/// its source and bound name) and is gone after that (contract section 4).
#[test]
fn codes_are_single_use() {
    let (_dir, app) = testing::app();
    let code = app
        .auth
        .mint_code(CodeSource::Improv, Some("Ana's iPhone".into()));
    assert_eq!(code.len(), CODE_CHARS);
    assert!(code.bytes().all(|b| CODE_ALPHABET.contains(&b)), "{code}");

    let displayed = format!("{}-{}", &code[..4], code[4..].to_lowercase());
    let taken = app.auth.take_code(&displayed).expect("live code");
    assert_eq!(taken.code, code);
    assert_eq!(taken.source, CodeSource::Improv);
    assert_eq!(taken.name.as_deref(), Some("Ana's iPhone"));
    assert_eq!(app.auth.take_code(&code), None);
}

/// A code is valid for CODE_LIFETIME_S (600 s) and no longer.
#[test]
fn codes_expire() {
    let (_dir, app) = testing::app();
    let fresh = app.auth.mint_code(CodeSource::New, None);
    let stale = app.auth.mint_code(CodeSource::New, None);
    app.auth.advance(CODE_LIFETIME_S - 1);
    assert!(app.auth.take_code(&fresh).is_some());
    app.auth.advance(1);
    assert_eq!(app.auth.take_code(&stale), None);
}

/// At most MAX_LIVE_CODES (16) live codes: the 17th evicts the oldest,
/// the rest stay usable.
#[test]
fn the_oldest_code_is_evicted() {
    let (_dir, app) = testing::app();
    let codes: Vec<String> = (0..=MAX_LIVE_CODES)
        .map(|_| app.auth.mint_code(CodeSource::Grant, None))
        .collect();
    assert_eq!(app.auth.take_code(&codes[0]), None);
    for code in &codes[1..] {
        assert!(app.auth.take_code(code).is_some(), "{code}");
    }
}

/// Names shown on the Dial and put in tokens (contract section 5): no
/// control characters or markup characters, trimmed, at most 48
/// characters with an ellipsis.
#[test]
fn device_names_are_sanitised() {
    assert_eq!(
        sanitize_name("  <b>Ana's\u{7} iPhone</b> & co ").as_deref(),
        Some("bAna's iPhone/b  co")
    );
    let long = "x".repeat(60);
    let shown = sanitize_name(&long).expect("name");
    assert_eq!(shown.chars().count(), 48);
    assert!(shown.ends_with('\u{2026}'));
    assert_eq!(sanitize_name("<>&\n "), None);
}

// ---- who is let in ----

/// Only the machine's own hotspot exempts its clients (contract section
/// 1): its IPv4 subnet, not the network a client joined, and nothing at
/// all when NetworkManager says the hotspot is off.
#[test]
fn only_the_hotspot_subnet_is_direct() {
    let hotspot = network(AP_CONNECTION, &["10.42.0.1/24"]).0.clone();
    let subnets = direct_subnets(&hotspot);
    assert_eq!(subnets, vec![IpNet::parse("10.42.0.1/24").expect("ip")]);
    for (ip, inside) in [
        ("10.42.0.7", true),
        ("::ffff:10.42.0.7", true),
        ("10.42.1.7", false),
        ("192.168.1.7", false),
        ("fe80::1", false),
    ] {
        let ip: IpAddr = ip.parse().expect("ip");
        assert_eq!(in_subnet(&subnets[0], ip), inside, "{ip}");
    }
    let home = network("HomeNet", &["10.42.0.1/24"]).0.clone();
    assert_eq!(direct_subnets(&home), Vec::new());
}

/// The handshake follows the HTTP rule (contract section 7): local and
/// hotspot sockets always connect, others need a token in the CONNECT
/// payload or an Authorization header while authorization is required,
/// and connect freely while it is off.
#[tokio::test]
async fn socket_handshakes_follow_the_http_rule() {
    let (_dir, app) = testing::app();
    let (token, _) = app.auth.issue("Phone").expect("issued");
    let remote = lan("192.168.1.42");

    let local = app
        .auth
        .admit_socket(&http::HeaderMap::new(), None)
        .await
        .expect("local");
    assert!(local.local);
    assert_eq!(app.auth.admit_socket(&remote, None).await, None);
    assert_eq!(
        app.auth
            .admit_socket(&remote, Some(&json!({"token": "forged"})))
            .await,
        None
    );
    let with_payload = app
        .auth
        .admit_socket(&remote, Some(&json!({"token": token})))
        .await
        .expect("token in the payload");
    assert_eq!(with_payload.token, Some((0, None)));
    let mut with_header = remote.clone();
    with_header.insert(
        http::header::AUTHORIZATION,
        format!("Bearer {token}").parse().expect("header"),
    );
    assert!(app.auth.admit_socket(&with_header, None).await.is_some());

    app.auth
        .set_network(network(AP_CONNECTION, &["10.42.0.1/24"]));
    let direct = app
        .auth
        .admit_socket(&lan("10.42.0.9"), None)
        .await
        .expect("hotspot client");
    assert!(direct.direct);

    app.config
        .set(keys::USER, keys::REQUIRE_AUTHORIZATION, json!(false));
    assert!(app.auth.admit_socket(&remote, None).await.is_some());
}

/// Which connected sockets the sweep keeps: after a sign-out only local
/// ones (contract section 3); once authorization turns on, local, hotspot
/// and those holding a token of the current secret that has not expired
/// (contract section 2).
#[test]
fn sweep_keeps_only_admitted_sockets() {
    let local = SocketAuth {
        local: true,
        direct: false,
        ip: None,
        token: None,
    };
    let direct = SocketAuth {
        local: false,
        direct: true,
        ip: Some("10.42.0.9".parse().expect("ip")),
        token: None,
    };
    let bare = SocketAuth {
        local: false,
        direct: false,
        ip: Some("192.168.1.42".parse().expect("ip")),
        token: None,
    };
    let current = SocketAuth {
        token: Some((1, None)),
        ..bare.clone()
    };
    let stale = SocketAuth {
        token: Some((0, None)),
        ..bare.clone()
    };
    let expiring = SocketAuth {
        token: Some((1, Some(100))),
        ..bare.clone()
    };
    let keeps = |state: &SocketAuth, signed_out: bool, required: bool, now: i64| {
        socket_may_stay(Some(state), signed_out, required, 1, now, &[])
    };

    assert!(keeps(&local, true, true, 0));
    for remote in [&direct, &bare, &current] {
        assert!(!keeps(remote, true, false, 0));
    }
    assert!(keeps(&direct, false, true, 0));
    assert!(keeps(&current, false, true, 0));
    assert!(keeps(&expiring, false, true, 99));
    assert!(!keeps(&expiring, false, true, 100));
    assert!(!keeps(&stale, false, true, 0));
    assert!(!keeps(&bare, false, true, 0));
    assert!(keeps(&bare, false, false, 0));
    assert!(!socket_may_stay(None, false, true, 1, 0, &[]));

    // A hotspot client that connected while authorization was off was not
    // checked then (the handshake skips NetworkManager): turning it on
    // keeps it while its address is in the hotspot's subnet.
    let unchecked = SocketAuth {
        direct: false,
        ..direct.clone()
    };
    let hotspot = direct_subnets(&network(AP_CONNECTION, &["10.42.0.1/24"]).0);
    assert!(socket_may_stay(
        Some(&unchecked),
        false,
        true,
        1,
        0,
        &hotspot
    ));
    assert!(!socket_may_stay(Some(&bare), false, true, 1, 0, &hotspot));
    assert!(!socket_may_stay(Some(&unchecked), false, true, 1, 0, &[]));
    assert!(!socket_may_stay(
        Some(&unchecked),
        true,
        true,
        1,
        0,
        &hotspot
    ));
}

/// A Socket.IO handshake checks the token once, so without a sweep at its
/// `exp` a socket keeps every broadcast and can still send actions after
/// HTTP started refusing the token (contract section 3: an expired token
/// is refused). The expected timing comes from the token's `exp`
/// (`system.auth_token_lifetime` after `iat`) and the one second
/// [`DeviceAuth::expiry_sweep_delay`] adds; tests have no socket to see
/// dropped, so the sweep shows as a disconnect pass on `Sio`.
#[tokio::test(start_paused = true)]
async fn sockets_are_swept_once_their_token_expires() {
    let (_dir, app) = testing::app();
    app.config
        .set(keys::SYSTEM, keys::AUTH_TOKEN_LIFETIME, json!(60));
    let (token, claims) = app.auth.issue("Phone").expect("issued");
    let exp = claims.exp.expect("an expiring token");
    let state = app
        .auth
        .admit_socket(&lan("192.168.1.42"), Some(&json!({"token": token})))
        .await
        .expect("admitted");
    assert_eq!(state.token, Some((0, Some(exp))));
    let delay = app.auth.expiry_sweep_delay(&state).expect("it expires");
    assert!((60..=61).contains(&delay.as_secs()), "{delay:?}");
    // Nothing expires for the machine itself or a token without `exp`.
    let local = app
        .auth
        .admit_socket(&http::HeaderMap::new(), None)
        .await
        .expect("local");
    assert_eq!(app.auth.expiry_sweep_delay(&local), None);
    let forever = SocketAuth {
        token: Some((0, None)),
        ..state.clone()
    };
    assert_eq!(app.auth.expiry_sweep_delay(&forever), None);

    tokio::time::sleep(Duration::from_secs(30)).await;
    assert_eq!(app.sio.disconnect_passes(), 0, "swept before exp");

    // The wall clock passes `exp` with the timer.
    app.auth.advance(61);
    assert_eq!(
        app.auth.expiry_sweep_delay(&state),
        Some(Duration::from_secs(1))
    );
    tokio::time::sleep(Duration::from_secs(40)).await;
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.sio.disconnect_passes() == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(app.sio.disconnect_passes(), 1);
}

/// Every tokenless LAN request asks whether the client is on the hotspot,
/// which reads NetworkManager (`nmcli`) when the cached subnets are older
/// than [`DIRECT_SUBNETS_TTL_S`]. A burst of them must start one read, not
/// one each, or an unpaired client can fill the blocking pool; and a failed
/// read keeps the last subnets for another TTL instead of being retried on
/// every request. The hotspot subnet comes from contract section 1.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hotspot_subnet_reads_are_single_flight() {
    let (_dir, app) = testing::app();
    let network = CountingNetwork::hotspot(Duration::from_millis(100));
    app.auth.set_network(network.clone());

    let requests: Vec<_> = (0..20)
        .map(|_| {
            let auth = app.auth.clone();
            tokio::spawn(async move { auth.admission(&lan("192.168.1.42")).await })
        })
        .collect();
    for request in requests {
        assert!(!request.await.expect("joined").admitted());
    }
    assert_eq!(network.reads(), 1);

    network.fail.store(true, Ordering::SeqCst);
    app.auth.advance(DIRECT_SUBNETS_TTL_S as u64 + 1);
    let hotspot_client = lan("10.42.0.9");
    assert!(app.auth.admission(&hotspot_client).await.direct);
    assert_eq!(network.reads(), 2);
    assert!(app.auth.admission(&hotspot_client).await.direct);
    assert_eq!(
        network.reads(),
        2,
        "a failed read is not retried within the TTL"
    );
}

/// With authorization off every socket connects, so the handshake has no
/// reason to read NetworkManager; it keeps the address for a later sweep.
#[tokio::test]
async fn open_handshakes_do_not_read_the_network() {
    let (_dir, app) = testing::app();
    let network = CountingNetwork::hotspot(Duration::ZERO);
    app.auth.set_network(network.clone());
    app.config
        .set(keys::USER, keys::REQUIRE_AUTHORIZATION, json!(false));

    let state = app
        .auth
        .admit_socket(&lan("10.42.0.9"), None)
        .await
        .expect("admitted");

    assert!(!state.direct);
    assert_eq!(state.ip, Some("10.42.0.9".parse().expect("ip")));
    assert_eq!(network.reads(), 0);
}
