//! Diagnostics for a Wi-Fi link that stays associated, with unicast
//! working, but stops receiving group-addressed (broadcast and multicast)
//! frames: phones then stop finding the machine over mDNS until it
//! reassociates or reboots. Reported in the field on IW612 machines on a
//! network that rotates its group key every hour. The cause is not proven: a
//! group key the driver or firmware failed to apply, or power save missing
//! the frames the AP buffers for the DTIM beacon, would look the same.
//!
//! Diagnostics only: nothing here touches the link, NetworkManager or the
//! driver. Every 30 s the monitor counts the broadcast frames other stations
//! sent us; after 10 minutes without one while unicast keeps arriving it
//! asks the network with a pair of DHCPINFORMs ([`probe`]), and it reports
//! each episode to Sentry once, when it ends, with the rekeys, the link and
//! the driver around it. The 24 h counters (`GET /api/v1/wifi/group-rx`)
//! are the denominator: how many rekeys and silences a machine saw.
//!
//! What it cannot tell: the IW612 exposes no CCMP decrypt or replay counters
//! and its driver registers no cfg80211 `get_key`, so "the old group key is
//! still in use", "frames arrive but are not decrypted" and "the AP stopped
//! sending" look the same from here. It can tell whether an episode followed
//! a rekey, and whether a later rekey, a roam or a reconnect ended it.

mod detector;
#[cfg(test)]
mod fake;
mod host;
mod parse;
mod probe;
mod report;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::{ConfigStore, WIFI_MODE_AP, keys};
use crate::sentry_reporting::EventSink;
use detector::{
    Detector, Ended, EndedBy, Firmware, Frames, Saved, Step, Thresholds, WINDOW, Window,
};
pub use host::{GroupRxHost, SystemGroupRxHost};
use parse::{IwLink, WpaEvent};
use probe::{Confidence, Inform, ProbeResult, Verdict};
use report::LinkFacts;

pub const IFACE: &str = "wlan0";
const DAY: f64 = 86_400.0;
/// How long to wait for each DHCP answer.
const PROBE_WAIT: f64 = 3.0;
/// A second round only when the first says broken or silent, so one lost
/// frame does not make an episode.
const PROBE_ATTEMPTS: usize = 2;
const MAX_EVENTS_PER_DAY: usize = 4;
const SAVE_EVERY: f64 = 600.0;
const LOG_LINES: usize = 200;
const LOG_BEFORE_START: f64 = 300.0;
const LOG_BEFORE_END: f64 = 600.0;
const MOAL_PARAMETERS: [&str; 5] = [
    "ps_mode",
    "auto_ds",
    "host_mlme",
    "gtk_rekey_offload",
    "drv_mode",
];
const STATE_VERSION: u32 = 1;

pub type Redact = Arc<dyn Fn(&str) -> String + Send + Sync>;

pub struct GroupRxMonitor {
    host: Arc<dyn GroupRxHost>,
    config: Arc<ConfigStore>,
    sink: Arc<dyn EventSink>,
    /// The log redaction, for the log lines attached to an event.
    redact: Redact,
    state_path: PathBuf,
    image_version: PathBuf,
    thresholds: Thresholds,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    detector: Detector,
    loaded: bool,
    running: bool,
    rx_packets: Option<u64>,
    firmware: Option<BTreeMap<String, u64>>,
    /// Wall time of the newest wpa_supplicant line read.
    journal_last: Option<f64>,
    link: Option<IwLink>,
    /// What the open episode will be reported with.
    extras: Option<Extras>,
    /// Wall times of the events sent in the last 24 h.
    sent: VecDeque<f64>,
    suppressed: u32,
    last_save: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Extras {
    facts: LinkFacts,
    start_log: Vec<String>,
}

/// The state file: survives backend restarts (the episode carries on) and
/// reboots (it is reported as ended by the reboot, and the 24 h counters
/// keep their history).
#[derive(Serialize, Deserialize)]
struct StateFile {
    version: u32,
    boot_id: String,
    detector: Saved,
    extras: Option<Extras>,
    sent: VecDeque<f64>,
    suppressed: u32,
}

impl GroupRxMonitor {
    pub fn new(
        host: Arc<dyn GroupRxHost>,
        config: Arc<ConfigStore>,
        sink: Arc<dyn EventSink>,
        redact: Redact,
        state_path: PathBuf,
        image_version: PathBuf,
    ) -> GroupRxMonitor {
        GroupRxMonitor {
            host,
            config,
            sink,
            redact,
            state_path,
            image_version,
            thresholds: Thresholds::default(),
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Tick every window on a thread of its own, for the life of the process.
    pub fn start(self: &Arc<Self>) {
        let me = self.clone();
        let spawned = std::thread::Builder::new()
            .name("WifiGroupRx".into())
            .spawn(move || {
                loop {
                    let next = me.host.boot_clock() + WINDOW;
                    me.tick();
                    me.host.sleep(next - me.host.boot_clock());
                }
            });
        if let Err(error) = spawned {
            tracing::warn!("Could not start the Wi-Fi group reception diagnostics: {error}");
        }
    }

    /// `user.wifi_group_rx_diagnostics`, and only as a client: in hotspot
    /// mode wlan0 is the access point.
    fn enabled(&self) -> bool {
        self.config
            .get_bool(keys::USER, keys::WIFI_GROUP_RX_DIAGNOSTICS)
            && self.config.get(keys::WIFI, keys::WIFI_MODE).as_str() != Some(WIFI_MODE_AP)
    }

    pub fn tick(&self) {
        let steps = {
            let mut inner = self.inner.lock();
            if !inner.loaded
                && let Some(ended) = self.load(&mut inner)
            {
                self.report(&mut inner, ended);
            }
            if !self.enabled() {
                self.turn_off(&mut inner);
                return;
            }
            if !inner.running {
                self.turn_on(&mut inner);
                return;
            }
            let window = self.collect(&mut inner);
            inner.detector.observe(&window)
        };
        for step in steps {
            match step {
                Step::Calibrate | Step::Probe => self.run_probe(),
                Step::Report(ended) => {
                    let mut inner = self.inner.lock();
                    self.report(&mut inner, *ended);
                }
            }
        }
        let mut inner = self.inner.lock();
        // An open episode is saved every window, so that a reboot reports
        // when it really ended.
        if inner.detector.episode().is_some()
            || self.host.boot_clock() - inner.last_save >= SAVE_EVERY
        {
            self.save(&mut inner);
        }
    }

    fn turn_on(&self, inner: &mut Inner) {
        self.host.start_sniffer(IFACE);
        // The first window starts now.
        let _ = self.host.take_frames();
        inner.running = true;
        inner.rx_packets = self.rx_packets();
        let module = self.module();
        let _ = self.firmware_delta(inner, &module);
        let wall = self.host.wall_clock();
        let rekeys: Vec<f64> = self
            .read_journal(inner, wall)
            .into_iter()
            .filter(|(_, event)| *event == WpaEvent::Rekey)
            .map(|(time, _)| time)
            .collect();
        inner.detector.backfill_rekeys(&rekeys);
        tracing::info!("Wi-Fi group reception diagnostics running on {IFACE}");
    }

    fn turn_off(&self, inner: &mut Inner) {
        if !inner.running {
            return;
        }
        self.host.stop_sniffer();
        inner.running = false;
        inner.link = None;
        tracing::info!("Wi-Fi group reception diagnostics stopped");
        if let Some(ended) = inner.detector.stop(self.host.boot_clock()) {
            self.report(inner, ended);
        }
        self.save(inner);
    }

    fn collect(&self, inner: &mut Inner) -> Window {
        let end = self.host.boot_clock();
        let wall = self.host.wall_clock();
        let wall_offset = wall - end;
        let group = self.host.take_frames();
        let link = self
            .host
            .run(&["iw", "dev", IFACE, "link"], 5.0)
            .and_then(|text| parse::iw_link(&text));
        let rx_packets = self.rx_packets();
        // A counter that went backwards (the driver reloaded) makes the
        // window unknown, like a sniffer gap.
        let received = match (rx_packets, inner.rx_packets) {
            (Some(now), Some(before)) => now.checked_sub(before),
            _ => None,
        };
        inner.rx_packets = rx_packets;
        let frames = group.zip(received).map(|(group, received)| Frames {
            broadcast: group.broadcast,
            multicast: group.multicast,
            unicast: received.saturating_sub(group.broadcast + group.multicast),
        });
        let mut rekeys = Vec::new();
        let mut connects = 0;
        for (time, event) in self.read_journal(inner, wall) {
            match event {
                WpaEvent::Rekey => rekeys.push(time - wall_offset),
                WpaEvent::Connected => connects += 1,
            }
        }
        let module = self.module();
        let firmware = self.firmware_delta(inner, &module);
        let window = Window {
            end,
            wall_offset,
            bssid: link.as_ref().map(|l| l.bssid.clone()),
            has_ipv4: self.host.ipv4(IFACE).is_some(),
            frames,
            rekeys,
            connects,
            firmware,
        };
        inner.link = link;
        window
    }

    fn rx_packets(&self) -> Option<u64> {
        self.host
            .read_file(&format!("/sys/class/net/{IFACE}/statistics/rx_packets"))?
            .trim()
            .parse()
            .ok()
    }

    fn module(&self) -> String {
        self.host
            .read_link(&format!("/sys/class/net/{IFACE}/device/driver/module"))
            .unwrap_or_default()
    }

    /// The wpa_supplicant events logged since the last read (the last 24 h
    /// on the first one), with their wall times.
    fn read_journal(&self, inner: &mut Inner, wall: f64) -> Vec<(f64, WpaEvent)> {
        let last = inner.journal_last.unwrap_or(wall - DAY);
        let since = format!("@{}", last.floor() as i64);
        let argv = [
            "journalctl",
            "-u",
            "wpa_supplicant",
            "-o",
            "short-unix",
            "--no-pager",
            "-q",
            "--since",
            &since,
        ];
        let Some(text) = self.host.run(&argv, 10.0) else {
            return Vec::new();
        };
        let mut newest = last;
        let events = text
            .lines()
            .filter_map(parse::journal_line)
            .filter(|line| line.time > last)
            .filter_map(|line| {
                newest = newest.max(line.time);
                parse::wpa_event(&line.message, IFACE).map(|event| (line.time, event))
            })
            .collect();
        inner.journal_last = Some(newest);
        events
    }

    /// The IW612 firmware's group-frame and missed-beacon counters over the
    /// window (`mlanutl` only works with the NXP driver).
    fn firmware_delta(&self, inner: &mut Inner, module: &str) -> Option<Firmware> {
        let now = (module == "moal")
            .then(|| self.host.run(&["mlanutl", IFACE, "getlog"], 5.0))
            .flatten()
            .map(|text| parse::getlog(&text));
        let before = std::mem::replace(&mut inner.firmware, now.clone());
        let (now, before) = (now?, before?);
        let delta = |key: &str| now.get(key)?.checked_sub(*before.get(key)?);
        Some(Firmware {
            group_frames: delta("dot11GroupReceivedFrameCount")?,
            beacons_missed: delta("beaconMissedCount")?,
        })
    }

    /// Run the probe the detector asked for, outside the lock: it takes a
    /// few seconds.
    fn run_probe(&self) {
        let result = self.probe();
        let opened = {
            let mut inner = self.inner.lock();
            let before = inner.detector.episode().is_some();
            inner.detector.probed(result);
            let episode = inner.detector.episode().filter(|_| !before).cloned();
            episode.map(|episode| (episode, inner.link.clone()))
        };
        tracing::info!(
            "Wi-Fi group reception probe: {} (calibration {})",
            probe::classify(&result).code(),
            self.inner.lock().detector.calibration().code()
        );
        let Some((episode, link)) = opened else {
            return;
        };
        let started_wall = episode.started + episode.wall_offset;
        let facts = self.facts(link.as_ref());
        let start_log = self.wifi_log(started_wall - LOG_BEFORE_START, self.host.wall_clock());
        tracing::warn!(
            "Wi-Fi group reception stall: no broadcast frame for {}s with unicast arriving; \
             probe {} ({} confidence)",
            (episode.confirmed - episode.started).round(),
            episode.verdict.code(),
            Confidence::of(episode.verdict, episode.calibration).code()
        );
        let mut inner = self.inner.lock();
        inner.extras = Some(Extras { facts, start_log });
        self.save(&mut inner);
    }

    /// Both INFORMs, flagged first, once more if the first round says
    /// broken or silent. `sent` stays 0 for an INFORM that could not go out.
    fn probe(&self) -> ProbeResult {
        let mut result = ProbeResult::default();
        let (Some(mac), Some(address)) = (self.own_mac(), self.host.ipv4(IFACE)) else {
            return result;
        };
        'attempts: for _ in 0..PROBE_ATTEMPTS {
            for broadcast in [true, false] {
                let xid = self.host.random_u32();
                let inform = Inform {
                    mac,
                    address,
                    xid,
                    broadcast,
                };
                match self.host.exchange(IFACE, &inform.frame(), xid, PROBE_WAIT) {
                    Ok(deliveries) if broadcast => result.flagged.add(&deliveries),
                    Ok(deliveries) => result.plain.add(&deliveries),
                    Err(error) => {
                        tracing::info!("Wi-Fi group reception probe could not send: {error}");
                        break 'attempts;
                    }
                }
            }
            if !matches!(
                probe::classify(&result),
                Verdict::GroupRxBroken | Verdict::ServerSilent
            ) {
                break;
            }
        }
        result
    }

    fn own_mac(&self) -> Option<[u8; 6]> {
        let text = self
            .host
            .read_file(&format!("/sys/class/net/{IFACE}/address"))?;
        let bytes = hex::decode(text.trim().replace(':', "")).ok()?;
        bytes.try_into().ok()
    }

    /// The link and driver as they are now.
    fn facts(&self, link: Option<&IwLink>) -> LinkFacts {
        let host = &*self.host;
        let mut facts = LinkFacts {
            module: self.module(),
            driver: host
                .read_link(&format!("/sys/class/net/{IFACE}/device/driver"))
                .unwrap_or_default(),
            os_version: host
                .read_file(&self.image_version.to_string_lossy())
                .map(|v| v.trim().to_string())
                .unwrap_or_default(),
            ..LinkFacts::default()
        };
        if facts.module == "moal" {
            let version = (0..4)
                .find_map(|n| host.read_file(&format!("/proc/mwlan/adapter{n}/{IFACE}/info")))
                .and_then(|text| parse::mwlan_version(&text));
            if let Some((chip, firmware, driver)) = version {
                facts.chip = chip;
                facts.firmware_version = firmware;
                facts.driver_version = driver;
            }
            for parameter in MOAL_PARAMETERS {
                if let Some(value) =
                    host.read_file(&format!("/sys/module/moal/parameters/{parameter}"))
                {
                    facts
                        .module_parameters
                        .insert(parameter.to_string(), value.trim().to_string());
                }
            }
        } else if let Some(text) = host.run(&["ethtool", "-i", IFACE], 5.0) {
            let mut fields = parse::fields(&text, ':', &["version", "firmware-version"]);
            facts.chip = facts.module.clone();
            facts.driver_version = fields.remove("version").unwrap_or_default();
            facts.firmware_version = fields.remove("firmware-version").unwrap_or_default();
        }
        if let Some(link) = link {
            if let Some((oui, local)) = parse::oui(&link.bssid) {
                facts.ap_oui = oui;
                facts.ap_oui_local = local;
            }
            facts.freq = link.freq;
            if let Some((band, channel)) = link.freq.and_then(parse::channel) {
                facts.band = band.to_string();
                facts.channel = Some(channel);
            }
            facts.signal_dbm = link.signal;
            facts.dtim_period = link.dtim_period;
            facts.beacon_interval = link.beacon_interval;
            let elements = host
                .run(&["wpa_cli", "-i", IFACE, "bss", &link.bssid], 5.0)
                .and_then(|text| parse::bss_elements(&text));
            if let Some(elements) = elements {
                facts.pmf = elements.pmf.unwrap_or_default();
                facts.akms = elements.akms;
                facts.ap_vendor_ouis = elements.vendor_ouis;
            }
        }
        if let Some(text) = host.run(&["wpa_cli", "-i", IFACE, "status"], 5.0) {
            let mut fields =
                parse::fields(&text, '=', &["key_mgmt", "group_cipher", "pairwise_cipher"]);
            facts.key_mgmt = fields.remove("key_mgmt").unwrap_or_default();
            facts.group_cipher = fields.remove("group_cipher").unwrap_or_default();
            facts.pairwise_cipher = fields.remove("pairwise_cipher").unwrap_or_default();
        }
        facts.power_save = host
            .run(&["iw", "dev", IFACE, "get", "power_save"], 5.0)
            .and_then(|text| parse::power_save(&text))
            .unwrap_or_default();
        facts
    }

    /// wpa_supplicant's lines and the kernel's Wi-Fi lines between two wall
    /// times, redacted, without the hostname, the last `LOG_LINES` of them.
    fn wifi_log(&self, from: f64, to: f64) -> Vec<String> {
        let since = format!("@{}", from.floor() as i64);
        let until = format!("@{}", to.ceil() as i64);
        let argv = [
            "journalctl",
            "-o",
            "short-unix",
            "--no-pager",
            "-q",
            "--since",
            &since,
            "--until",
            &until,
            "_SYSTEMD_UNIT=wpa_supplicant.service",
            "+",
            "_TRANSPORT=kernel",
        ];
        let Some(text) = self.host.run(&argv, 15.0) else {
            return Vec::new();
        };
        let mut lines: Vec<String> = text
            .lines()
            .filter_map(parse::journal_line)
            .filter(|line| line.identifier != "kernel" || parse::is_wifi_kernel_line(&line.message))
            .map(|line| {
                (self.redact)(&format!(
                    "{} {}: {}",
                    report::log_time(line.time),
                    line.identifier,
                    line.message
                ))
            })
            .collect();
        let excess = lines.len().saturating_sub(LOG_LINES);
        lines.drain(..excess);
        lines
    }

    /// One event per episode, at most `MAX_EVENTS_PER_DAY` a day; the
    /// episodes over the limit are counted in the next event.
    fn report(&self, inner: &mut Inner, ended: Ended) {
        let episode = &ended.episode;
        let extras = if ended.ended_by == EndedBy::StillOngoing {
            inner.extras.clone()
        } else {
            inner.extras.take()
        }
        .unwrap_or_default();
        tracing::warn!(
            "Wi-Fi group reception stall ended by {} after {}s: {} rekeys, {} unicast frames, \
             probe {}",
            ended.ended_by.code(),
            (ended.end - episode.started).round(),
            episode.tally.rekeys,
            episode.tally.unicast,
            episode.verdict.code()
        );
        let now_wall = self.host.wall_clock();
        while inner.sent.front().is_some_and(|t| *t < now_wall - DAY) {
            inner.sent.pop_front();
        }
        if inner.sent.len() >= MAX_EVENTS_PER_DAY {
            inner.suppressed += 1;
            tracing::info!("Wi-Fi group reception stall not reported: rate limit reached");
            self.save(inner);
            return;
        }
        let end_wall = ended.end + episode.wall_offset;
        let end_log = self.wifi_log(end_wall - LOG_BEFORE_END, end_wall + 1.0);
        let now = self.host.boot_clock();
        let counters = inner.detector.counters(now, now_wall - now);
        let event = report::event(&report::Input {
            ended: &ended,
            facts: &extras.facts,
            start_log: &extras.start_log,
            end_log: &end_log,
            counters: &counters,
            thresholds: &self.thresholds,
            suppressed: inner.suppressed,
        });
        self.sink.send(event);
        inner.sent.push_back(now_wall);
        inner.suppressed = 0;
        self.save(inner);
    }

    fn boot_id(&self) -> Option<String> {
        self.host
            .read_file("/proc/sys/kernel/random/boot_id")
            .map(|id| id.trim().to_string())
    }

    fn load(&self, inner: &mut Inner) -> Option<Ended> {
        inner.loaded = true;
        let text = std::fs::read_to_string(&self.state_path).ok()?;
        let file: StateFile = match serde_json::from_str(&text) {
            Ok(file) => file,
            Err(error) => {
                tracing::warn!("Ignoring the Wi-Fi group reception state: {error}");
                return None;
            }
        };
        if file.version != STATE_VERSION {
            return None;
        }
        let same_boot = self.boot_id().is_some_and(|id| id == file.boot_id);
        let (detector, ended) = Detector::restore(self.thresholds, file.detector, same_boot);
        inner.detector = detector;
        inner.extras = file.extras;
        inner.sent = file.sent;
        inner.suppressed = file.suppressed;
        ended
    }

    fn save(&self, inner: &mut Inner) {
        inner.last_save = self.host.boot_clock();
        let file = StateFile {
            version: STATE_VERSION,
            boot_id: self.boot_id().unwrap_or_default(),
            detector: inner.detector.saved().clone(),
            extras: inner.extras.clone(),
            sent: inner.sent.clone(),
            suppressed: inner.suppressed,
        };
        let write = || -> std::io::Result<()> {
            let text = serde_json::to_string(&file).map_err(std::io::Error::other)?;
            if let Some(parent) = self.state_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let temporary = self.state_path.with_extension("tmp");
            std::fs::write(&temporary, text)?;
            std::fs::rename(&temporary, &self.state_path)
        };
        if let Err(error) = write() {
            tracing::warn!("Could not save the Wi-Fi group reception state: {error}");
        }
    }

    /// The 24 h counters and the current state, for the device attributes.
    pub fn status(&self) -> Value {
        let enabled = self.enabled();
        let inner = self.inner.lock();
        let now = self.host.boot_clock();
        let wall_offset = self.host.wall_clock() - now;
        let counters = inner.detector.counters(now, wall_offset);
        let episode = inner.detector.episode();
        let state = if !enabled {
            "off"
        } else if !inner.running {
            "starting"
        } else if episode.is_some() {
            "episode"
        } else if inner.link.is_none() {
            "not_associated"
        } else {
            "ok"
        };
        json!({
            "enabled": enabled,
            "state": state,
            "quiet_for_s": inner.detector.quiet_for(now).map(|s| s.max(0.0).round() as u64),
            "calibration": inner.detector.calibration().code(),
            "window_s": DAY as u64,
            "covered_s": counters.covered_s,
            "rekeys": counters.rekeys,
            "episodes": counters.episodes,
            "longest_silence_s": counters.longest_silence_s,
            "probes": counters.probes,
            "false_alarms": counters.false_alarms,
            "rekey_interval_s": counters.rekey_interval_s,
            "events_sent": inner.sent.len(),
            "events_suppressed": inner.suppressed,
            "episode": episode.map(|e| json!({
                "started_at": report::date(e.started + e.wall_offset),
                "duration_s": (now - e.started).max(0.0).round() as u64,
                "verdict": e.verdict.code(),
                "confidence": Confidence::of(e.verdict, e.calibration).code(),
            })),
        })
    }
}
