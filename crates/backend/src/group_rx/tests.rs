//! The group reception diagnostics against a scripted IW612 machine: when a
//! silence becomes a probe, a probe an episode and an episode a Sentry
//! event, and that the monitor never reports a quiet network, an idle link
//! or the same episode twice. Expected values come from the thresholds in
//! `detector.rs`, the probe rules in `probe.rs` and the office captures
//! of 2026-11-03 the fake host replays.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};

use super::detector::{Detector, EndedBy, Frames, Step, Thresholds, WINDOW, Window};
use super::fake::{BSSID, FakeHost, RecordingSink, WALL_OFFSET};
use super::probe::{Delivery, ProbeResult, Replies};
use super::{GroupRxMonitor, report};
use crate::config::{self, ConfigStore, WIFI_MODE_AP, WIFI_MODE_CLIENT, keys};
use crate::redaction::Redaction;
use crate::sentry_reporting::EventReport;

/// 20 windows of 30 s: `suspect_after`.
const SILENCE: usize = 20;
const WINDOW_S: u64 = WINDOW as u64;

struct Bench {
    dir: tempfile::TempDir,
    host: Arc<FakeHost>,
    sink: Arc<RecordingSink>,
    config: Arc<ConfigStore>,
    redaction: Arc<Redaction>,
    monitor: GroupRxMonitor,
}

impl Bench {
    /// A client-mode machine with the diagnostics on (the default), turned
    /// on by a first tick.
    fn new() -> Bench {
        let dir = tempfile::tempdir().expect("temporary directory");
        let config = Arc::new(ConfigStore::open(
            dir.path().join("config.yml"),
            config::defaults(),
        ));
        config.set(keys::WIFI, keys::WIFI_MODE, json!(WIFI_MODE_CLIENT));
        let redaction = Redaction::new(dir.path().join("redaction_key"));
        redaction.refresh_values(&json!({"wifi": {"KnownWifis": {"HomeNet": {}}}}));
        let host = Arc::new(FakeHost::iw612());
        host.state.lock().files.insert(
            "/opt/image-build-version".into(),
            "2026M1514-nightly\n".into(),
        );
        let sink = Arc::new(RecordingSink::default());
        let monitor = monitor(&dir, &host, &sink, &config, &redaction);
        let bench = Bench {
            dir,
            host,
            sink,
            config,
            redaction,
            monitor,
        };
        bench.monitor.tick();
        bench
    }

    /// The backend restarts (or the machine reboots): a new monitor on the
    /// same state file, turned on by its first tick.
    fn restart(&mut self) {
        self.monitor = monitor(
            &self.dir,
            &self.host,
            &self.sink,
            &self.config,
            &self.redaction,
        );
        self.monitor.tick();
    }

    fn run(&self, windows: usize, broadcast: u64, unicast: u64) {
        for _ in 0..windows {
            self.host.window(broadcast, 3, unicast);
            self.monitor.tick();
        }
    }

    /// Healthy until calibrated, then broadcasts stop right after a rekey
    /// and the next probe shows the stall signature.
    fn open_episode(&self) {
        self.run(4, 100, 50);
        self.host.answers(vec![], vec![Delivery::Unicast]);
        self.host.rekey();
        self.run(SILENCE, 0, 40);
        assert!(self.monitor.inner.lock().detector.episode().is_some());
    }

    fn events(&self) -> Vec<EventReport> {
        self.sink.events.lock().clone()
    }

    fn status(&self) -> Value {
        self.monitor.status()
    }
}

fn monitor(
    dir: &tempfile::TempDir,
    host: &Arc<FakeHost>,
    sink: &Arc<RecordingSink>,
    config: &Arc<ConfigStore>,
    redaction: &Arc<Redaction>,
) -> GroupRxMonitor {
    let redaction = redaction.clone();
    GroupRxMonitor::new(
        host.clone(),
        config.clone(),
        sink.clone(),
        Arc::new(move |text: &str| redaction.redact_text(text)),
        dir.path().join("syslog/wifi-group-rx.json"),
        PathBuf::from("/opt/image-build-version"),
    )
}

fn tag<'a>(event: &'a EventReport, key: &str) -> &'a str {
    event
        .tags
        .iter()
        .find(|(k, _)| k == key)
        .map_or("", |(_, v)| v.as_str())
}

fn context<'a>(event: &'a EventReport, name: &str) -> &'a Value {
    event
        .contexts
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v)
        .expect("context")
}

/// The case from the field: broadcasts stop right after a group rekey while
/// unicast keeps flowing, and come back with a later rekey. One event, when
/// it ends, carrying what tells the hypotheses apart (rekey timing, the
/// probe's two answers, power save, DTIM, driver and firmware), with the
/// attached logs redacted.
#[test]
fn stall_after_a_rekey_is_confirmed_by_the_probe_and_reported_once_when_a_rekey_heals_it() {
    let bench = Bench::new();
    bench.open_episode();
    assert_eq!(
        bench.host.informs(),
        [true, false, true, false, true, false],
        "calibration (one round, it worked), then two rounds that showed the stall"
    );
    assert!(bench.events().is_empty(), "reported when it ends");
    assert_eq!(bench.status()["state"], "episode");

    bench.run(10, 0, 40);
    bench.host.rekey();
    bench.run(1, 80, 40);

    let events = bench.events();
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event.message, report::MESSAGE);
    assert_eq!(event.level, sentry::Level::Warning);
    assert_eq!(event.fingerprint, ["wifi-group-rx-stall", "moal"]);
    for (key, value) in [
        ("wifi.driver", "moal"),
        ("wifi.chip", "SDIW612"),
        ("wifi.firmware", "18.99.3.p21.10"),
        ("wifi.driver_version", "MM6X18505.p14"),
        ("os.image", "2026M1514-nightly"),
        ("wifi.ap_oui", "82:83:c2"),
        ("wifi.band", "5GHz"),
        ("wifi.channel", "36"),
        ("wifi.security", "WPA2-PSK"),
        ("wifi.pmf", "off"),
        ("wifi.power_save", "on"),
        ("wifi.dtim", "3"),
        ("group_rx.ended_by", "rekey"),
        ("group_rx.verdict", "group_rx_broken"),
        ("group_rx.confidence", "high"),
        ("group_rx.calibration", "broadcasts"),
        ("group_rx.after_rekey", "yes"),
    ] {
        assert_eq!(tag(event, key), value, "{key}");
    }
    let details = context(event, "wifi_group_rx");
    assert_eq!(details["duration_s"], (SILENCE as u64 + 11) * WINDOW_S);
    assert_eq!(details["confirmed_after_s"], SILENCE as u64 * WINDOW_S);
    assert_eq!(details["rekey_before_start_s"], 0);
    assert_eq!(
        details["rekeys_during"], 2,
        "the one before the silence and the healing one"
    );
    assert_eq!(details["unicast_frames_during"], (SILENCE as u64 + 11) * 40);
    assert_eq!(
        details["multicast_frames_during"],
        (SILENCE as u64 + 11) * 3
    );
    assert_eq!(details["firmware_group_frames_during"], 0);
    assert_eq!(
        details["probe"],
        json!({
            "broadcast_flag": {"sent": 2, "broadcast": 0, "multicast": 0, "unicast": 0},
            "unicast_flag": {"sent": 2, "broadcast": 0, "multicast": 0, "unicast": 2},
        })
    );
    assert_eq!(details["started_at"], report::date(WALL_OFFSET + 1120.0));
    let link = context(event, "wifi_link");
    assert_eq!(link["ap_vendor_ouis"], json!(["00:15:6d"]));
    assert_eq!(
        link["module_parameters"],
        json!({"auto_ds": "1", "ps_mode": "1"})
    );
    assert!(
        !link.to_string().contains(BSSID),
        "the AP is known by its OUI only"
    );

    let names: Vec<&str> = event.attachments.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["wifi-log-start.txt", "wifi-log-end.txt"]);
    for (_, text) in &event.attachments {
        assert!(text.contains("SME: Trying to authenticate"), "{text}");
        assert!(
            text.contains("kernel: wlan: Received disassociation"),
            "{text}"
        );
        assert!(!text.contains("usb 1-1"), "only Wi-Fi kernel lines: {text}");
        assert!(
            !text.contains("HomeNet"),
            "network names are redacted: {text}"
        );
        assert!(!text.contains("meticulousMachine"), "no hostname: {text}");
    }
    assert_eq!(bench.status()["episodes"], 1);
    assert_eq!(bench.status()["state"], "ok");
}

/// A network that is just quiet: the probe gets the broadcast answer, so
/// nothing is reported and the next probe waits twice as long.
#[test]
fn a_quiet_network_is_not_an_episode_and_probes_back_off() {
    let bench = Bench::new();
    bench.run(4, 100, 50);
    let calibration = bench.host.informs().len();
    bench.run(SILENCE, 0, 40);
    assert_eq!(
        bench.host.informs().len(),
        calibration + 2,
        "one probe round"
    );
    bench.run(SILENCE - 1, 0, 40);
    assert_eq!(
        bench.host.informs().len(),
        calibration + 2,
        "backed off to 20 min"
    );
    bench.run(1, 0, 40);
    assert_eq!(bench.host.informs().len(), calibration + 4);
    assert!(bench.events().is_empty());
    let status = bench.status();
    assert_eq!(status["false_alarms"], 2);
    assert_eq!(status["episodes"], 0);
    assert_eq!(status["longest_silence_s"], 2 * SILENCE as u64 * WINDOW_S);
}

/// Silence without unicast is an idle or dead link: the Wi-Fi health check
/// handles that, this signature needs unicast working.
#[test]
fn silence_without_unicast_is_never_probed() {
    let bench = Bench::new();
    bench.run(4, 100, 50);
    let calibration = bench.host.informs().len();
    bench.run(4 * SILENCE, 0, 0);
    assert_eq!(bench.host.informs().len(), calibration);
}

/// A server that answers no INFORM leaves only the passive count: still an
/// episode, at low confidence, here ended by the link going down.
#[test]
fn a_silent_server_gives_a_low_confidence_episode() {
    let bench = Bench::new();
    bench.host.answers(vec![], vec![]);
    bench.run(4, 100, 50);
    assert_eq!(bench.status()["calibration"], "silent");
    bench.run(SILENCE, 0, 40);
    bench.host.associate(None);
    bench.run(1, 0, 0);
    let events = bench.events();
    assert_eq!(events.len(), 1);
    assert_eq!(tag(&events[0], "group_rx.verdict"), "server_silent");
    assert_eq!(tag(&events[0], "group_rx.confidence"), "low");
    assert_eq!(tag(&events[0], "group_rx.ended_by"), "disconnected");
    assert_eq!(tag(&events[0], "group_rx.after_rekey"), "unknown");
    assert_eq!(bench.status()["state"], "not_associated");
}

/// The customer's way out was a reboot: the episode is saved when it opens
/// and reported on the next boot as ended by the reboot, with the facts and
/// the start log it was saved with.
#[test]
fn an_episode_a_reboot_ends_is_reported_on_the_next_boot() {
    let mut bench = Bench::new();
    bench.open_episode();
    bench.run(2, 0, 40);
    let last_window = bench.host.now();
    {
        // Ten minutes later, 40 s into the new boot.
        let mut state = bench.host.state.lock();
        state.wall_offset += last_window + 600.0 - 40.0;
        state.boot = 40.0;
        state.files.insert(
            "/proc/sys/kernel/random/boot_id".into(),
            "a-new-boot\n".into(),
        );
        state.links.clear();
    }
    bench.restart();
    let events = bench.events();
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(tag(event, "group_rx.ended_by"), "reboot");
    assert_eq!(
        tag(event, "wifi.chip"),
        "SDIW612",
        "facts from the state file"
    );
    assert_eq!(
        context(event, "wifi_group_rx")["ended_at"],
        report::date(WALL_OFFSET + last_window)
    );
    assert!(
        event
            .attachments
            .iter()
            .any(|(n, _)| n == "wifi-log-start.txt")
    );
    assert_eq!(bench.status()["episodes"], 1, "the 24 h history survives");
}

/// A backend restart (the daily one) is not the end of an episode: it
/// carries on and is reported once, with its real start.
#[test]
fn an_episode_survives_a_backend_restart() {
    let mut bench = Bench::new();
    bench.open_episode();
    bench.restart();
    assert!(bench.events().is_empty());
    bench.run(3, 0, 40);
    bench.run(1, 60, 40);
    let events = bench.events();
    assert_eq!(events.len(), 1);
    assert_eq!(tag(&events[0], "group_rx.ended_by"), "spontaneous");
    assert_eq!(
        context(&events[0], "wifi_group_rx")["duration_s"],
        (SILENCE as u64 + 4) * WINDOW_S
    );
}

/// Turning the setting off stops the sniffer and the probes at once; an
/// open episode is reported as stopped. Hotspot mode never starts them.
#[test]
fn the_setting_and_hotspot_mode_turn_the_monitor_off() {
    let bench = Bench::new();
    bench.open_episode();
    bench
        .config
        .set(keys::USER, keys::WIFI_GROUP_RX_DIAGNOSTICS, json!(false));
    bench.run(1, 0, 40);
    assert!(!bench.host.state.lock().sniffing);
    assert_eq!(bench.events().len(), 1);
    assert_eq!(tag(&bench.events()[0], "group_rx.ended_by"), "stopped");
    let informs = bench.host.informs().len();
    bench.run(3 * SILENCE, 0, 40);
    assert_eq!(bench.host.informs().len(), informs);
    assert_eq!(bench.status()["state"], "off");

    bench
        .config
        .set(keys::USER, keys::WIFI_GROUP_RX_DIAGNOSTICS, json!(true));
    bench
        .config
        .set(keys::WIFI, keys::WIFI_MODE, json!(WIFI_MODE_AP));
    bench.run(3 * SILENCE, 0, 40);
    assert!(!bench.host.state.lock().sniffing);
    assert_eq!(bench.host.informs().len(), informs);
}

/// At most four events a day; the ones over the limit are counted.
#[test]
fn reports_are_rate_limited() {
    let bench = Bench::new();
    bench.run(4, 100, 50);
    bench.host.answers(vec![], vec![Delivery::Unicast]);
    for _ in 0..6 {
        bench.run(SILENCE, 0, 40);
        bench.run(1, 50, 40);
    }
    assert_eq!(bench.events().len(), 4);
    assert_eq!(bench.status()["events_suppressed"], 2);
    assert_eq!(bench.status()["episodes"], 6);
}

/// A stall that never heals is reported after six hours, once.
#[test]
fn a_long_episode_is_reported_as_ongoing_and_not_again() {
    let bench = Bench::new();
    bench.open_episode();
    let six_hours = (6.0 * 3600.0 / WINDOW) as usize;
    bench.run(six_hours - SILENCE - 1, 0, 40);
    assert!(bench.events().is_empty());
    bench.run(1, 0, 40);
    assert_eq!(bench.events().len(), 1);
    assert_eq!(
        tag(&bench.events()[0], "group_rx.ended_by"),
        "still_ongoing"
    );
    bench.run(1, 70, 40);
    assert_eq!(bench.events().len(), 1);
}

/// The denominators: rekeys (the journal of the day before the monitor
/// started included), the interval between them and the covered time.
#[test]
fn counters_cover_the_last_day() {
    let bench = Bench::new();
    for _ in 0..6 {
        bench.host.rekey();
        bench.run(20, 100, 50);
    }
    let status = bench.status();
    assert_eq!(status["rekeys"], 6);
    assert_eq!(status["rekey_interval_s"], 600);
    assert_eq!(status["covered_s"], 120 * WINDOW_S);
    assert_eq!(status["episodes"], 0);
    assert_eq!(status["state"], "ok");
    assert_eq!(status["window_s"], 86400);

    // Restarted: what was logged before comes from the journal once, not
    // twice.
    let mut bench = bench;
    bench.restart();
    assert_eq!(bench.status()["rekeys"], 6);
}

/// The probe could not be sent (no socket, no address): only the passive
/// count is left, at low confidence.
#[test]
fn a_probe_that_cannot_be_sent_leaves_the_passive_count() {
    let bench = Bench::new();
    bench.run(4, 100, 50);
    bench.host.state.lock().send_fails = true;
    bench.run(SILENCE, 0, 40);
    bench.run(1, 10, 40);
    let events = bench.events();
    assert_eq!(events.len(), 1);
    assert_eq!(tag(&events[0], "group_rx.verdict"), "send_failed");
    assert_eq!(tag(&events[0], "group_rx.confidence"), "low");
}

fn window(end: f64, broadcast: u64) -> Window {
    Window {
        end,
        wall_offset: WALL_OFFSET,
        bssid: Some(BSSID.into()),
        has_ipv4: true,
        frames: Some(Frames {
            broadcast,
            multicast: 0,
            unicast: 10,
        }),
        ..Window::default()
    }
}

fn broken() -> ProbeResult {
    ProbeResult {
        flagged: Replies {
            sent: 2,
            ..Replies::default()
        },
        plain: Replies {
            sent: 2,
            unicast: 2,
            ..Replies::default()
        },
    }
}

/// One window, with a calibration it asks for answered as on the office
/// network (both answers, the flagged one broadcast).
fn feed(detector: &mut Detector, w: &Window) -> Vec<Step> {
    let steps = detector.observe(w);
    if steps.contains(&Step::Calibrate) {
        detector.probed(ProbeResult {
            flagged: Replies {
                sent: 1,
                broadcast: 1,
                ..Replies::default()
            },
            plain: Replies {
                sent: 1,
                unicast: 1,
                ..Replies::default()
            },
        });
    }
    steps
}

/// Drive a detector into an open episode; returns the time of the last
/// window.
fn episode(detector: &mut Detector) -> f64 {
    let mut t = 0.0;
    for _ in 0..3 {
        t += WINDOW;
        feed(detector, &window(t, 50));
    }
    loop {
        t += WINDOW;
        if feed(detector, &window(t, 0)).contains(&Step::Probe) {
            detector.probed(broken());
            return t;
        }
    }
}

fn ended_by(steps: &[Step]) -> Option<EndedBy> {
    steps.iter().find_map(|step| match step {
        Step::Report(ended) => Some(ended.ended_by),
        _ => None,
    })
}

/// How an episode ended decides between "a rekey heals it" (a lost group
/// key), "only a new association does" (state stuck in the driver) and
/// "it went away by itself".
#[test]
fn how_an_episode_ends() {
    let mut detector = Detector::new(Thresholds::default());
    let t = episode(&mut detector);
    let mut roam = window(t + WINDOW, 40);
    roam.bssid = Some("d0:21:f9:ed:2a:8a".into());
    assert_eq!(ended_by(&detector.observe(&roam)), Some(EndedBy::Roam));

    let mut detector = Detector::new(Thresholds::default());
    let t = episode(&mut detector);
    let mut reconnect = window(t + WINDOW, 40);
    reconnect.connects = 1;
    assert_eq!(
        ended_by(&detector.observe(&reconnect)),
        Some(EndedBy::Reconnect)
    );

    let mut detector = Detector::new(Thresholds::default());
    let t = episode(&mut detector);
    let mut rekey = window(t + WINDOW, 0);
    rekey.rekeys = vec![t + 20.0];
    assert_eq!(ended_by(&detector.observe(&rekey)), None, "still silent");
    assert_eq!(
        ended_by(&detector.observe(&window(t + 2.0 * WINDOW, 40))),
        Some(EndedBy::Rekey),
        "broadcasts back in the window after the rekey"
    );

    let mut detector = Detector::new(Thresholds::default());
    let t = episode(&mut detector);
    assert_eq!(
        ended_by(&detector.observe(&window(t + WINDOW, 40))),
        Some(EndedBy::Spontaneous)
    );
}

/// A window the sniffer did not cover (socket reopened after a driver
/// reload) proves nothing: the silence starts again after it.
#[test]
fn an_uncovered_window_restarts_the_silence() {
    let mut detector = Detector::new(Thresholds::default());
    let mut t = 0.0;
    for _ in 0..3 {
        t += WINDOW;
        feed(&mut detector, &window(t, 50));
    }
    for i in 0..(2 * SILENCE) {
        t += WINDOW;
        let mut w = window(t, 0);
        if i == SILENCE - 2 {
            w.frames = None;
        }
        let steps = feed(&mut detector, &w);
        assert_eq!(
            steps.contains(&Step::Probe),
            i == 2 * SILENCE - 2,
            "window {i}"
        );
    }
}

/// Saved state comes back within the same boot; after a reboot the open
/// episode ends, unless its ongoing report already went out.
#[test]
fn restore_within_a_boot_and_after_a_reboot() {
    let mut detector = Detector::new(Thresholds::default());
    let t = episode(&mut detector);
    let saved = detector.saved().clone();

    let (same, ended) = Detector::restore(Thresholds::default(), saved.clone(), true);
    assert!(ended.is_none());
    assert!(same.episode().is_some());

    let (after, ended) = Detector::restore(Thresholds::default(), saved, false);
    let ended = ended.expect("the reboot ends it");
    assert_eq!(ended.ended_by, EndedBy::Reboot);
    assert_eq!(ended.end, t);
    assert!(after.episode().is_none());

    let mut detector = Detector::new(Thresholds {
        report_ongoing_after: 900.0,
        ..Thresholds::default()
    });
    let mut t = episode(&mut detector);
    let mut reported = false;
    while !reported {
        t += WINDOW;
        reported = ended_by(&detector.observe(&window(t, 0))) == Some(EndedBy::StillOngoing);
    }
    let (_, ended) = Detector::restore(Thresholds::default(), detector.saved().clone(), false);
    assert!(ended.is_none());
}

/// The route the hawkBit attribute publisher reads, local only.
#[tokio::test]
async fn the_counters_route_is_local_only() {
    let (_dir, app) = crate::testing::app();
    let reply = crate::testing::call(&app, http::Method::GET, "/api/v1/wifi/group-rx", "").await;
    assert_eq!(reply.status_code(), 200);
    let body = crate::testing::json(&reply);
    assert_eq!(body["enabled"], false, "the default mode is the hotspot");
    assert_eq!(body["state"], "off");
    assert_eq!(body["episodes"], 0);
    let remote = crate::testing::call_with_headers(
        &app,
        http::Method::GET,
        "/api/v1/wifi/group-rx",
        "",
        &[("X-Real-IP", "192.168.1.20")],
    )
    .await;
    assert_eq!(remote.status_code(), 403);
}
