//! The Sentry event for one episode: grouped as one issue per driver, with
//! the conditions it happened under as tags and the details as contexts.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::detector::{Counters, Ended, Thresholds};
use super::probe::{Confidence, Replies};
use crate::sentry_reporting::EventReport;

pub const MESSAGE: &str = "Wi-Fi stopped receiving broadcast frames while unicast kept working";
/// A silence that starts this soon after a rekey "follows" it.
const AFTER_REKEY: f64 = 120.0;

/// The link and driver when the episode was confirmed. No network name, no
/// full BSSID: the AP is known by its OUI only.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LinkFacts {
    /// The kernel module behind wlan0 (`moal`, `brcmfmac`).
    pub module: String,
    /// The bus driver (`wlan_sdio`, `brcmfmac`).
    pub driver: String,
    pub chip: String,
    pub driver_version: String,
    pub firmware_version: String,
    /// The NXP module's power-save and key-offload parameters.
    pub module_parameters: BTreeMap<String, String>,
    pub os_version: String,
    pub ap_oui: String,
    /// A locally administered BSSID, whose OUI names no vendor.
    pub ap_oui_local: bool,
    pub ap_vendor_ouis: Vec<String>,
    pub band: String,
    pub channel: Option<u32>,
    pub freq: Option<u32>,
    pub signal_dbm: Option<i32>,
    pub key_mgmt: String,
    pub group_cipher: String,
    pub pairwise_cipher: String,
    pub akms: Vec<String>,
    pub pmf: String,
    pub power_save: String,
    pub dtim_period: Option<u32>,
    pub beacon_interval: Option<u32>,
}

/// Everything one report says.
pub struct Input<'a> {
    pub ended: &'a Ended,
    pub facts: &'a LinkFacts,
    pub start_log: &'a [String],
    pub end_log: &'a [String],
    pub counters: &'a Counters,
    pub thresholds: &'a Thresholds,
    /// Episodes not reported in the last 24 h because of the rate limit.
    pub suppressed: u32,
}

pub fn event(input: &Input<'_>) -> EventReport {
    let Input {
        ended,
        facts,
        counters,
        ..
    } = input;
    let episode = &ended.episode;
    let confidence = Confidence::of(episode.verdict, episode.calibration);
    let after_rekey = match episode.rekey_before_start {
        Some(seconds) if seconds <= AFTER_REKEY => "yes",
        Some(_) => "no",
        None => "unknown",
    };
    let tags = [
        ("wifi.driver", facts.module.clone()),
        ("wifi.chip", facts.chip.clone()),
        ("wifi.driver_version", facts.driver_version.clone()),
        ("wifi.firmware", facts.firmware_version.clone()),
        ("os.image", facts.os_version.clone()),
        ("wifi.ap_oui", facts.ap_oui.clone()),
        ("wifi.band", facts.band.clone()),
        ("wifi.channel", optional(facts.channel)),
        ("wifi.security", facts.key_mgmt.clone()),
        ("wifi.pmf", facts.pmf.clone()),
        ("wifi.power_save", facts.power_save.clone()),
        ("wifi.dtim", optional(facts.dtim_period)),
        ("group_rx.ended_by", ended.ended_by.code().to_string()),
        ("group_rx.verdict", episode.verdict.code().to_string()),
        ("group_rx.confidence", confidence.code().to_string()),
        (
            "group_rx.calibration",
            episode.calibration.code().to_string(),
        ),
        ("group_rx.after_rekey", after_rekey.to_string()),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value))
    .collect();

    let tally = &episode.tally;
    let replies = |r: &Replies| json!({"sent": r.sent, "broadcast": r.broadcast, "multicast": r.multicast, "unicast": r.unicast});
    let context = json!({
        "started_at": date(episode.started + episode.wall_offset),
        "ended_at": date(ended.end + episode.wall_offset),
        "duration_s": (ended.end - episode.started).max(0.0).round() as u64,
        "confirmed_after_s": (episode.confirmed - episode.started).max(0.0).round() as u64,
        "ended_by": ended.ended_by.code(),
        "verdict": episode.verdict.code(),
        "confidence": confidence.code(),
        "calibration": episode.calibration.code(),
        "probe": {
            "broadcast_flag": replies(&episode.probe.flagged),
            "unicast_flag": replies(&episode.probe.plain),
        },
        "rekey_before_start_s": episode.rekey_before_start.map(|s| s.round() as u64),
        "rekeys_during": tally.rekeys,
        "rekey_interval_s": counters.rekey_interval_s,
        "roams_during": tally.roams,
        "reconnects_during": tally.reconnects,
        "multicast_frames_during": tally.multicast,
        "unicast_frames_during": tally.unicast,
        "firmware_group_frames_during": tally.firmware.map(|f| f.group_frames),
        "beacons_missed_during": tally.firmware.map(|f| f.beacons_missed),
        "suspect_after_s": input.thresholds.suspect_after.round() as u64,
        "events_suppressed": input.suppressed,
        "counters_24h": counters,
    });
    let link = serde_json::to_value(facts).unwrap_or(Value::Null);

    let attachments = [
        ("wifi-log-start.txt", input.start_log),
        ("wifi-log-end.txt", input.end_log),
    ]
    .into_iter()
    .filter(|(_, lines)| !lines.is_empty())
    .map(|(name, lines)| (name.to_string(), lines.join("\n") + "\n"))
    .collect();

    EventReport {
        message: MESSAGE.to_string(),
        level: sentry::Level::Warning,
        fingerprint: vec![
            "wifi-group-rx-stall".to_string(),
            if facts.module.is_empty() {
                "unknown".to_string()
            } else {
                facts.module.clone()
            },
        ],
        tags,
        contexts: vec![
            ("wifi_group_rx".to_string(), context),
            ("wifi_link".to_string(), link),
        ],
        attachments,
    }
}

fn optional(value: Option<u32>) -> String {
    value.map(|v| v.to_string()).unwrap_or_default()
}

/// RFC 3339 in UTC, to the second.
pub fn date(wall: f64) -> String {
    chrono::DateTime::from_timestamp(wall.floor() as i64, 0)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

/// RFC 3339 in UTC, to the millisecond, for log lines.
pub fn log_time(wall: f64) -> String {
    let millis = (wall * 1000.0).floor() as i64;
    chrono::DateTime::from_timestamp_millis(millis)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_default()
}
