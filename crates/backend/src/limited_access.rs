//! `limited_access.py`: a machine on the `factory` update channel only shows
//! the built-in profile until it is unlocked with the master code or the code
//! derived from its serial number.

use std::collections::VecDeque;
use std::time::Instant;

use parking_lot::Mutex;
use serde_json::{Value, json};

use crate::app::App;
use crate::config::{ConfigStore, keys};
use crate::ota::py_strip;
use crate::profiles::ProfileEvent;

pub const LIMITED_ACCESS_CHANNEL: &str = "factory";
pub const UNLOCKED_CHANNEL: &str = "stable";
pub const MASTER_UNLOCK_CODE: &str = "met";
pub const UNLOCK_CODE_XOR: u64 = 0xC0FFEE;
pub const UNLOCK_CODE_MODULO: u64 = 99999;
pub const UNLOCK_MAX_FAILURES: usize = 10;
pub const UNLOCK_FAILURE_WINDOW_S: f64 = 60.0;
pub const UNLOCK_LOCKOUT_S: f64 = 30.0;

/// `is_limited_access`.
pub fn is_limited_access(config: &ConfigStore) -> bool {
    config.get(keys::USER, keys::UPDATE_CHANNEL) == json!(LIMITED_ACCESS_CHANNEL)
}

/// `_JS_INT_PREFIX.match(str(value))`: optional whitespace, an optional sign
/// and ASCII digits, like JavaScript's `parseInt`. Returns the sign and the
/// digits.
fn int_prefix(text: &str) -> Option<(bool, &str)> {
    let rest = text.trim_start_matches(|c: char| crate::ota::py_isspace(c));
    let (negative, rest) = match rest.as_bytes().first() {
        Some(b'-') => (true, &rest[1..]),
        Some(b'+') => (false, &rest[1..]),
        _ => (false, rest),
    };
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    (digits > 0).then(|| (negative, &rest[..digits]))
}

/// `serial_unlock_code`: `(int(serial) ^ 0xC0FFEE) % 99999` for a
/// non-negative integer prefix of the serial, on Python's unbounded ints.
pub fn serial_unlock_code(serial: &Value) -> Option<String> {
    if serial.is_null() {
        return None;
    }
    let text = crate::ota::python_str(serial);
    let (negative, digits) = int_prefix(&text)?;
    let is_zero = digits.bytes().all(|b| b == b'0');
    if negative && !is_zero {
        return None;
    }
    // The XOR only touches the low 24 bits, so the serial modulo
    // 2^24 * 99999 decides the result for any length of digits.
    const LOW_BITS: u128 = 1 << 24;
    let modulus = LOW_BITS * u128::from(UNLOCK_CODE_MODULO);
    let mut rest: u128 = 0;
    for digit in digits.bytes() {
        rest = (rest * 10 + u128::from(digit - b'0')) % modulus;
    }
    let low = rest % LOW_BITS;
    let high = (rest - low) % u128::from(UNLOCK_CODE_MODULO);
    let mixed = (low ^ u128::from(UNLOCK_CODE_XOR)) % u128::from(UNLOCK_CODE_MODULO);
    Some(((high + mixed) % u128::from(UNLOCK_CODE_MODULO)).to_string())
}

/// `expected_unlock_codes`.
pub fn expected_unlock_codes(config: &ConfigStore) -> Vec<String> {
    let mut codes = vec![MASTER_UNLOCK_CODE.to_string()];
    if let Some(code) = serial_unlock_code(&config.get(keys::SYSTEM, keys::MACHINE_SERIAL_NUMBER)) {
        codes.push(code);
    }
    codes
}

/// `validate_unlock_code`.
pub fn validate_unlock_code(config: &ConfigStore, code: &Value) -> bool {
    let Value::String(code) = code else {
        return false;
    };
    let wanted = py_strip(code).to_lowercase();
    expected_unlock_codes(config).contains(&wanted)
}

struct Throttle {
    failures: VecDeque<f64>,
    locked_until: f64,
}

/// `UnlockThrottle`: more than ten failures within a minute lock the unlock
/// endpoint for 30 seconds. Times are seconds on a monotonic clock.
pub struct UnlockThrottle {
    epoch: Instant,
    state: Mutex<Throttle>,
}

impl Default for UnlockThrottle {
    fn default() -> UnlockThrottle {
        UnlockThrottle {
            epoch: Instant::now(),
            state: Mutex::new(Throttle {
                failures: VecDeque::new(),
                locked_until: 0.0,
            }),
        }
    }
}

impl UnlockThrottle {
    /// `time.monotonic()`: seconds since the throttle was created.
    pub fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    pub fn reset(&self) {
        let mut state = self.state.lock();
        state.failures.clear();
        state.locked_until = 0.0;
    }

    /// `retry_after`: whole seconds to wait, rounded up; 0 when unlocked.
    pub fn retry_after(&self, now: f64) -> u64 {
        let remaining = self.state.lock().locked_until - now;
        if remaining > 0.0 {
            remaining as u64 + 1
        } else {
            0
        }
    }

    pub fn record_failure(&self, now: f64) {
        let mut state = self.state.lock();
        state.failures.push_back(now);
        while state
            .failures
            .front()
            .is_some_and(|first| now - first > UNLOCK_FAILURE_WINDOW_S)
        {
            state.failures.pop_front();
        }
        if state.failures.len() > UNLOCK_MAX_FAILURES {
            state.locked_until = now + UNLOCK_LOCKOUT_S;
            state.failures.clear();
            tracing::warn!(
                "Unlock endpoint locked for {} s after repeated failures",
                UNLOCK_LOCKOUT_S as u64
            );
        }
    }
}

/// `unlock`: leave the factory channel for stable and have clients reload
/// the profile list. Returns the channel now configured.
pub fn unlock(app: &App) -> Value {
    if !is_limited_access(&app.config) {
        return app.config.get(keys::USER, keys::UPDATE_CHANNEL);
    }
    app.ota.set_channel(UNLOCKED_CHANNEL);
    app.config
        .set(keys::USER, keys::UPDATE_CHANNEL, json!(UNLOCKED_CHANNEL));
    app.config.save();
    // ProfileManager._emit_profile_event(PROFILE_EVENT.RELOAD).
    app.sio.emit(
        "profile",
        &crate::profiles::profile_event_payload(ProfileEvent::Reload, None, None, None),
    );
    tracing::info!("Machine unlocked: update channel set to {UNLOCKED_CHANNEL}");
    json!(UNLOCKED_CHANNEL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{app, call, json as body};

    fn js_code(serial: u64) -> String {
        ((serial ^ 0xC0FFEE) % 99999).to_string()
    }

    /// tests/test_limited_access.py::test_serial_unlock_code_matches_javascript_parse_int
    #[test]
    fn serial_unlock_code_matches_javascript_parse_int() {
        for (serial, expected) in [
            (json!("12345"), Some(js_code(12345))),
            (json!(12345), Some(js_code(12345))),
            (json!(" 42abc"), Some(js_code(42))),
            (json!("ME1234"), None),
            (json!("\u{661}\u{662}\u{663}"), None),
            (json!("NOT_ASSIGNED"), None),
            (Value::Null, None),
            (json!(""), None),
            (json!("-42"), None),
        ] {
            assert_eq!(serial_unlock_code(&serial), expected, "{serial}");
        }
        // Python ints have no width: a long serial still has a code.
        let long = "123456789012345678901234567890";
        let big: u128 = long.parse().expect("fits u128");
        let expected = ((big ^ 0xC0FFEE) % 99999).to_string();
        assert_eq!(serial_unlock_code(&json!(long)), Some(expected));
        assert_eq!(serial_unlock_code(&json!("-0")), Some(js_code(0)));
    }

    /// tests/test_limited_access.py::test_validate_unlock_code_accepts_master_and_serial_code
    #[test]
    fn validate_unlock_code_accepts_master_and_serial_code() {
        let (_dir, app) = app();
        app.config
            .set(keys::SYSTEM, keys::MACHINE_SERIAL_NUMBER, json!("12345"));
        assert!(validate_unlock_code(&app.config, &json!("met")));
        assert!(validate_unlock_code(&app.config, &json!(" MET ")));
        assert!(validate_unlock_code(&app.config, &json!(js_code(12345))));
        assert!(!validate_unlock_code(&app.config, &json!("")));
        assert!(!validate_unlock_code(&app.config, &Value::Null));
        assert!(!validate_unlock_code(&app.config, &json!("00000")));
    }

    /// tests/test_limited_access.py::test_limited_access_depends_only_on_update_channel
    #[test]
    fn limited_access_depends_only_on_update_channel() {
        let (_dir, app) = app();
        app.config
            .set(keys::USER, keys::UPDATE_CHANNEL, json!("factory"));
        assert!(is_limited_access(&app.config));
        app.config
            .set(keys::USER, keys::UPDATE_CHANNEL, json!("stable"));
        assert!(!is_limited_access(&app.config));
    }

    /// tests/test_limited_access.py::test_throttle_locks_after_more_than_ten_failures
    #[test]
    fn throttle_locks_after_more_than_ten_failures() {
        let throttle = UnlockThrottle::default();
        for _ in 0..UNLOCK_MAX_FAILURES {
            throttle.record_failure(100.0);
        }
        assert_eq!(throttle.retry_after(100.0), 0);
        throttle.record_failure(100.0);
        let wait = throttle.retry_after(100.0);
        assert!(wait > 0 && wait <= UNLOCK_LOCKOUT_S as u64 + 1);
        assert_eq!(throttle.retry_after(131.0), 0);
    }

    async fn post_unlock(app: &std::sync::Arc<App>, body: &str) -> crate::http::Reply {
        crate::testing::call_with_headers(
            app,
            http::Method::POST,
            "/api/v1/machine/unlock",
            body.to_string(),
            &[("Content-Type", "application/json")],
        )
        .await
    }

    /// tests/test_limited_access.py::LimitedAccessApiTests::test_unlock_statuses_and_side_effects
    #[tokio::test]
    async fn unlock_statuses_and_side_effects() {
        let (dir, app) = app();
        std::fs::create_dir_all(dir.path().join("etc/hawkbit")).expect("hawkbit");
        app.config
            .set(keys::USER, keys::UPDATE_CHANNEL, json!("factory"));
        app.config
            .set(keys::SYSTEM, keys::MACHINE_SERIAL_NUMBER, json!("12345"));
        let saves = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = saves.clone();
        app.config.on_save(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        });

        assert_eq!(post_unlock(&app, "{").await.status_code(), 400);
        let malformed = crate::testing::call_with_headers(
            &app,
            http::Method::POST,
            "/api/v1/machine/unlock",
            bytes::Bytes::from_static(b"\x80"),
            &[("Content-Type", "application/json")],
        )
        .await;
        assert_eq!(malformed.status_code(), 400);
        assert_eq!(body(&malformed)["data"]["code"], json!("INVALID_BODY"));
        assert_eq!(
            post_unlock(&app, r#"{"code": "00000"}"#)
                .await
                .status_code(),
            403
        );
        let reply = post_unlock(&app, &json!({"code": js_code(12345)}).to_string()).await;
        assert_eq!(reply.status_code(), 200);
        assert_eq!(
            body(&reply),
            json!({"status": "ok", "update_channel": "stable", "limited_access": false})
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("etc/hawkbit/channel")).expect("channel"),
            "stable\n"
        );
        assert_eq!(saves.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(
            app.config.get(keys::USER, keys::UPDATE_CHANNEL),
            json!("stable")
        );
    }

    /// tests/test_limited_access.py::LimitedAccessApiTests::test_unlock_throttles_after_eleven_failed_attempts
    #[tokio::test]
    async fn unlock_throttles_after_eleven_failed_attempts() {
        let (_dir, app) = app();
        for _ in 0..=UNLOCK_MAX_FAILURES {
            assert_eq!(
                post_unlock(&app, r#"{"code": "00000"}"#)
                    .await
                    .status_code(),
                403
            );
        }
        let reply = post_unlock(&app, r#"{"code": "met"}"#).await;
        assert_eq!(reply.status_code(), 429);
        assert!(reply.header("Retry-After").is_some_and(|v| !v.is_empty()));
        assert_eq!(body(&reply)["data"]["code"], json!("UNLOCK_THROTTLED"));
    }

    /// tests/test_limited_access.py::LimitedAccessApiTests::test_limited_list_defaults_and_get:
    /// a factory machine lists, offers and serves only the bundled Simple
    /// profile, whatever profiles the user has.
    #[tokio::test]
    async fn limited_list_defaults_and_get() {
        const SIMPLE: &str = "f5db85cc-ab0d-4c81-aee8-bcac7da77141";
        let (_dir, app) = app();
        let bundled = std::fs::read_to_string(&app.env.paths.simple_profile).expect("bundled");
        let mut user: Value = serde_json::from_str(&bundled).expect("JSON");
        user["id"] = json!("0d6f3a52-7c1e-4b8e-9a51-3f2b6c9d1e47");
        user["name"] = json!("User");
        std::fs::create_dir_all(&app.env.paths.profiles).expect("profiles");
        std::fs::write(
            app.env
                .paths
                .profiles
                .join("0d6f3a52-7c1e-4b8e-9a51-3f2b6c9d1e47.json"),
            user.to_string(),
        )
        .expect("user profile");
        app.profiles.refresh_profile_list(&app);
        let get = |uri: &'static str| call(&app, http::Method::GET, uri, "");
        let listed = body(&get("/api/v1/profile/list").await);
        assert_eq!(listed[0]["name"], json!("User"), "{listed}");

        app.config
            .set(keys::USER, keys::UPDATE_CHANNEL, json!("factory"));

        let profiles = body(&get("/api/v1/profile/list?full=true").await);
        assert_eq!(profiles.as_array().map(Vec::len), Some(1), "{profiles}");
        assert_eq!(profiles[0]["id"], json!(SIMPLE));
        assert!(
            profiles[0]["stages"]
                .as_array()
                .is_some_and(|s| !s.is_empty())
        );
        let summaries = body(&get("/api/v1/profile/list").await);
        assert!(summaries[0].get("stages").is_none(), "{summaries}");
        let defaults = body(&get("/api/v1/profile/defaults").await);
        assert_eq!(defaults["default"].as_array().map(Vec::len), Some(1));
        assert_eq!(defaults["community"], json!([]));
        let reply = call(
            &app,
            http::Method::GET,
            &format!("/api/v1/profile/get/{SIMPLE}"),
            "",
        )
        .await;
        assert_eq!(reply.status_code(), 200);
    }

    /// An empty or non-object body, a blank code: 400 before the throttle.
    #[tokio::test]
    async fn unlock_rejects_bodies_without_a_code() {
        let (_dir, app) = app();
        for body_text in ["", "[]", r#"{"code": "  "}"#, r#"{"code": 5}"#, "null"] {
            let reply = call(
                &app,
                http::Method::POST,
                "/api/v1/machine/unlock",
                body_text,
            )
            .await;
            assert_eq!(reply.status_code(), 400, "{body_text}");
            assert_eq!(
                body(&reply),
                json!({"status": "error", "error": "body must be {\"code\": string}",
                       "data": {"code": "INVALID_BODY"}})
            );
        }
        // Not limited: the configured channel comes back unchanged.
        let reply = call(
            &app,
            http::Method::POST,
            "/api/v1/machine/unlock",
            r#"{"code":"met"}"#,
        )
        .await;
        assert_eq!(
            body(&reply),
            json!({"status": "ok", "update_channel": "", "limited_access": false})
        );
    }
}
