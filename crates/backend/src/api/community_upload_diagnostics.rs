//! `api/community_upload_diagnostics.py`: the community uploader's status
//! for bug reports, bounded and allowlisted; never its credentials or queued
//! bodies.

use std::io::Read;
use std::path::Path;

use serde_json::{Map, Value, json};

pub const NAME: &str = "community_upload_status.json";
pub const MAX_BYTES: usize = 16384;

const CATEGORIES: [&str; 51] = [
    "upload_network",
    "token_network",
    "exchange_network",
    "disconnect_network",
    "machine_history_unavailable",
    "machine_pour_over_history_unavailable",
    "machine_history_invalid",
    "machine_history_too_large",
    "machine_history_incomplete",
    "machine_pour_over_history_invalid",
    "machine_pour_over_history_too_large",
    "machine_pour_over_history_incomplete",
    "response_read_failed",
    "response_too_large",
    "state_unavailable",
    "state_persist_failed",
    "queue_write_failed",
    "queue_capacity_reached",
    "queued_body_missing",
    "queued_body_too_large",
    "shot_file_pending",
    "shot_file_too_large",
    "shot_data_invalid",
    "shot_id_invalid",
    "history_path_invalid",
    "key_missing",
    "private_key_invalid",
    "header_invalid",
    "enrollment_missing",
    "pairing_code_expired",
    "invalid_exchange_body",
    "invalid_json",
    "invalid_payload",
    "expired_request",
    "replayed_request",
    "retired_or_revoked_access",
    "idempotency_payload_mismatch",
    "community_pour_over_not_ready",
    "pour_over_replay_scheduled",
    "pour_over_samples_invalid",
    "pour_over_duration_invalid",
    "pour_over_mode_invalid",
    "pour_over_pours_invalid",
    "pour_over_schema_invalid",
    "pour_over_targets_invalid",
    "pour_over_type_invalid",
    "upload_failed",
    "token_failed",
    "exchange_failed",
    "rate_limited",
    "other_error",
];

const RECOVERY_STATES: [&str; 9] = [
    "idle",
    "pending",
    "scanning",
    "uploading",
    "completed",
    "paused",
    "failed",
    "interrupted",
    "running",
];

/// `_number`: an `int` (not a bool) in `0..=2**63-1`, else `None`.
fn number(value: Option<&Value>) -> Value {
    match value {
        Some(Value::Number(n)) if !n.is_f64() => match n.as_i64() {
            Some(i) if i >= 0 => json!(i),
            _ => Value::Null,
        },
        _ => Value::Null,
    }
}

/// `_category`: `None` stays `None`, an unknown value is `other_error`.
fn category(value: Option<&Value>) -> Value {
    match value {
        None | Some(Value::Null) => Value::Null,
        Some(Value::String(s)) if CATEGORIES.contains(&s.as_str()) => json!(s),
        Some(_) => json!("other_error"),
    }
}

fn boolean(value: Option<&Value>) -> Value {
    match value {
        Some(Value::Bool(b)) => json!(b),
        _ => Value::Null,
    }
}

/// `int(text, 16)`: surrounding whitespace, a sign and single underscores
/// between digits are accepted.
fn python_hex_int(text: &str) -> Option<(bool, u128)> {
    let text = crate::bug_report::py::strip(text);
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
    {
        return None;
    }
    let cleaned: String = digits.chars().filter(|c| *c != '_').collect();
    if !cleaned.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u128::from_str_radix(&cleaned, 16)
        .ok()
        .map(|v| (negative, v))
}

/// `str(UUID(value))`, or `None` where `UUID()` raises.
fn canonical_uuid(value: Option<&Value>) -> Value {
    let Some(Value::String(text)) = value else {
        return Value::Null;
    };
    let hex = text.replace("urn:", "").replace("uuid:", "");
    let hex = hex.trim_matches(|c| c == '{' || c == '}').replace('-', "");
    if hex.chars().count() != 32 {
        return Value::Null;
    }
    match python_hex_int(&hex) {
        Some((negative, v)) if !negative || v == 0 => {
            let h = format!("{v:032x}");
            json!(format!(
                "{}-{}-{}-{}-{}",
                &h[0..8],
                &h[8..12],
                &h[12..16],
                &h[16..20],
                &h[20..32]
            ))
        }
        _ => Value::Null,
    }
}

/// `json.loads(bytes)`: a UTF-8 BOM is allowed.
fn loads(raw: &[u8]) -> Option<Value> {
    let raw = raw.strip_prefix(b"\xef\xbb\xbf").unwrap_or(raw);
    let text = std::str::from_utf8(raw).ok()?;
    pyfmt::loads_lenient(text).ok()
}

/// `collect()`: the snapshot under `<config>/community-upload/diagnostics.json`.
pub fn collect(config_dir: &Path, now: i64) -> Value {
    let mut result = Map::new();
    result.insert("schemaVersion".into(), json!(1));
    result.insert("collectedAt".into(), json!(now));
    result.insert("available".into(), json!(false));
    let source = config_dir.join("community-upload").join("diagnostics.json");
    let raw = match std::fs::File::open(&source) {
        Ok(file) => {
            let mut raw = Vec::new();
            match file.take(MAX_BYTES as u64 + 1).read_to_end(&mut raw) {
                Ok(_) => raw,
                Err(_) => {
                    result.insert("reason".into(), json!("unreadable"));
                    return Value::Object(result);
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            result.insert("reason".into(), json!("missing"));
            return Value::Object(result);
        }
        Err(_) => {
            result.insert("reason".into(), json!("unreadable"));
            return Value::Object(result);
        }
    };
    if raw.len() > MAX_BYTES {
        result.insert("reason".into(), json!("oversized"));
        return Value::Object(result);
    }
    let Some(data) = loads(&raw) else {
        result.insert("reason".into(), json!("unreadable"));
        return Value::Object(result);
    };
    let supported = data.as_object().is_some_and(|d| {
        matches!(d.get("schemaVersion"), Some(Value::Number(n)) if !n.is_f64() && n.as_i64() == Some(1))
    });
    if !supported {
        result.insert("reason".into(), json!("unsupported_schema"));
        return Value::Object(result);
    }
    result.insert("available".into(), json!(true));
    for key in ["capturedAt", "pendingCount", "lastSuccessAt", "lastRetryAt"] {
        result.insert(key.into(), number(data.get(key)));
    }
    let stale = match result.get("capturedAt").and_then(Value::as_i64) {
        None => true,
        Some(captured) => !(0..=120).contains(&(now - captured)),
    };
    result.insert("stale".into(), json!(stale));
    for key in ["connected", "paused"] {
        result.insert(key.into(), boolean(data.get(key)));
    }
    result.insert("lastError".into(), category(data.get("lastError")));
    let recovery = match data.get("recovery") {
        Some(Value::Object(recovery)) if recovery.get("available") == Some(&json!(true)) => {
            let mut selected = Map::new();
            selected.insert("available".into(), json!(true));
            let state = match recovery.get("state") {
                Some(Value::String(s)) if RECOVERY_STATES.contains(&s.as_str()) => json!(s),
                _ => Value::Null,
            };
            selected.insert("state".into(), state);
            for key in [
                "added",
                "alreadyPresent",
                "preservedDeleted",
                "failed",
                "pendingCount",
            ] {
                selected.insert(key.into(), number(recovery.get(key)));
            }
            selected.insert("lastError".into(), category(recovery.get("lastError")));
            Value::Object(selected)
        }
        _ => json!({"available": false}),
    };
    result.insert("recovery".into(), recovery);
    let last_failure = match data.get("lastFailure") {
        Some(Value::Object(failure)) => {
            let status = number(failure.get("httpStatus"))
                .as_i64()
                .filter(|s| (100..=599).contains(s));
            pyfmt::object([
                ("category", category(failure.get("category"))),
                ("occurredAt", number(failure.get("occurredAt"))),
                ("httpStatus", status.map_or(Value::Null, |s| json!(s))),
                (
                    "retryAfterSeconds",
                    number(failure.get("retryAfterSeconds")),
                ),
                ("requestId", canonical_uuid(failure.get("requestId"))),
            ])
        }
        _ => Value::Null,
    };
    result.insert("lastFailure".into(), last_failure);
    Value::Object(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_dir() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("community-upload").join("diagnostics.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        (dir, path)
    }

    fn now() -> i64 {
        chrono::Utc::now().timestamp()
    }

    /// tests/test_community_upload_diagnostics.py::test_snapshot_allowlist
    #[test]
    fn snapshot_allowlist() {
        let (dir, path) = snapshot_dir();
        std::fs::write(
            &path,
            pyfmt::dumps_default(&json!({
                "schemaVersion": 1,
                "capturedAt": 1,
                "connected": true,
                "pendingCount": 2,
                "privateSeed": "SECRET",
                "lastError": "SECRET",
                "recovery": {
                    "available": true,
                    "state": "running",
                    "added": 3,
                    "accessToken": "SECRET",
                },
                "lastFailure": {
                    "category": "upload_network",
                    "httpStatus": 503,
                    "requestId": "SECRET",
                    "body": "SECRET",
                },
            })),
        )
        .unwrap();
        let result = collect(dir.path(), now());
        assert!(!pyfmt::dumps_default(&result).contains("SECRET"));
        assert_eq!(result["available"], true);
        assert_eq!(result["stale"], true);
        assert_eq!(result["pendingCount"], 2);
        assert_eq!(result["recovery"]["added"], 3);
        assert_eq!(result["lastFailure"]["httpStatus"], 503);
        assert_eq!(result["lastFailure"]["requestId"], Value::Null);
        assert_eq!(result["lastError"], "other_error");
    }

    /// tests/test_community_upload_diagnostics.py::test_unavailable_snapshot
    /// (missing, unreadable, oversized, unsupported_schema).
    #[test]
    fn unavailable_snapshot() {
        let oversized = "x".repeat(MAX_BYTES + 1);
        for (content, reason) in [
            (None, "missing"),
            (Some("{"), "unreadable"),
            (Some(oversized.as_str()), "oversized"),
            (Some("[]"), "unsupported_schema"),
        ] {
            let (dir, path) = snapshot_dir();
            if let Some(content) = content {
                std::fs::write(&path, content).unwrap();
            }
            let result = collect(dir.path(), now());
            assert_eq!(result["available"], false, "{reason}");
            assert_eq!(result["reason"], reason);
        }
    }

    /// tests/test_community_upload_diagnostics.py::test_current_snapshot_and_correlation_id
    #[test]
    fn current_snapshot_and_correlation_id() {
        let (dir, path) = snapshot_dir();
        let request_id = "08c2cbe7-112b-4145-baf3-e5d29df2e455";
        std::fs::write(
            &path,
            pyfmt::dumps_default(&json!({
                "schemaVersion": 1,
                "capturedAt": now(),
                "lastFailure": {
                    "category": "upload_failed",
                    "httpStatus": 401,
                    "requestId": request_id,
                },
            })),
        )
        .unwrap();
        let result = collect(dir.path(), now());
        assert_eq!(result["stale"], false);
        assert_eq!(result["lastFailure"]["requestId"], request_id);
    }

    /// The key order and value types `collect()` writes.
    #[test]
    fn full_snapshot_keeps_python_key_order() {
        let (dir, path) = snapshot_dir();
        std::fs::write(&path, "{\"schemaVersion\": 1, \"capturedAt\": true}").unwrap();
        let result = collect(dir.path(), 100);
        assert_eq!(
            pyfmt::dumps_default(&result),
            "{\"schemaVersion\": 1, \"collectedAt\": 100, \"available\": true, \"capturedAt\": null, \"pendingCount\": null, \"lastSuccessAt\": null, \"lastRetryAt\": null, \"stale\": true, \"connected\": null, \"paused\": null, \"lastError\": null, \"recovery\": {\"available\": false}, \"lastFailure\": null}"
        );
    }

    /// `UUID()` accepts braces, a `urn:uuid:` prefix and upper case.
    #[test]
    fn request_ids_are_canonicalized_like_uuid() {
        assert_eq!(
            canonical_uuid(Some(&json!("{08C2CBE7112B4145BAF3E5D29DF2E455}"))),
            json!("08c2cbe7-112b-4145-baf3-e5d29df2e455")
        );
        assert_eq!(
            canonical_uuid(Some(&json!(
                "urn:uuid:08c2cbe7-112b-4145-baf3-e5d29df2e455"
            ))),
            json!("08c2cbe7-112b-4145-baf3-e5d29df2e455")
        );
        assert_eq!(canonical_uuid(Some(&json!(5))), Value::Null);
        assert_eq!(canonical_uuid(Some(&json!("zz"))), Value::Null);
    }
}
