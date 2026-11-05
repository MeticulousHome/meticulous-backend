//! The episode state machine, fed one window at a time. Pure: the monitor
//! collects the windows, runs the probes this asks for and feeds back their
//! results.
//!
//! Silence is counted on broadcast frames. A stall of all group traffic
//! silences them, and so does a broadcast-only one on an AP that converts
//! multicast to unicast (which then arrives under the pairwise key); the
//! multicast frames seen during an episode are reported to tell the two
//! apart. The probe, not the silence, decides: a network can be quiet.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use super::probe::{self, Calibration, ProbeResult, Verdict};

/// Seconds per window.
pub const WINDOW: f64 = 30.0;
const DAY: f64 = 86_400.0;
/// A rekey this close before the end of a silence is what ended it.
const HEALED_BY_REKEY_WITHIN: f64 = 2.0 * WINDOW;
/// Gaps between rekeys longer than this are disconnections, not the
/// network's interval.
const MAX_REKEY_INTERVAL: f64 = 6.0 * 3600.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// Silence, with unicast arriving, before the probe runs. Conservative
    /// on purpose: home networks carry ARP and mDNS broadcasts every few
    /// seconds (the office: ~100 broadcast frames per 30 s).
    pub suspect_after: f64,
    /// The longest the wait doubles to after probes found group reception
    /// working (a network with little broadcast traffic).
    pub max_suspect_after: f64,
    /// Minimum time between two probes.
    pub probe_interval: f64,
    /// How long after associating the calibration runs.
    pub calibrate_after: f64,
    /// An episode still open after this long is reported as ongoing.
    pub report_ongoing_after: f64,
}

impl Default for Thresholds {
    fn default() -> Thresholds {
        Thresholds {
            suspect_after: 600.0,
            max_suspect_after: 4.0 * 3600.0,
            probe_interval: 600.0,
            calibrate_after: 60.0,
            report_ongoing_after: 6.0 * 3600.0,
        }
    }
}

/// What one window saw. Times are on the boot clock, which does not jump
/// when NTP sets the wall clock.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Window {
    pub end: f64,
    /// Wall clock minus boot clock, to date the window.
    pub wall_offset: f64,
    /// The AP we are associated with, `None` when not associated.
    pub bssid: Option<String>,
    pub has_ipv4: bool,
    /// What the sniffer counted; `None` when it did not cover the window.
    pub frames: Option<Frames>,
    /// Boot-clock times of the group rekeys wpa_supplicant completed.
    pub rekeys: Vec<f64>,
    /// Associations wpa_supplicant completed (CTRL-EVENT-CONNECTED).
    pub connects: u32,
    /// The IW612 firmware's counters over the window.
    pub firmware: Option<Firmware>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Frames {
    pub broadcast: u64,
    pub multicast: u64,
    /// Received frames that were neither (interface counter minus both).
    pub unicast: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Firmware {
    /// `dot11GroupReceivedFrameCount`, counted after decryption and
    /// filtering: equal to what reaches the host on a healthy link.
    pub group_frames: u64,
    pub beacons_missed: u64,
}

/// What happened since the last broadcast frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tally {
    pub rekeys: u32,
    pub roams: u32,
    pub reconnects: u32,
    pub multicast: u64,
    pub unicast: u64,
    pub firmware: Option<Firmware>,
}

impl Tally {
    fn add(&mut self, window: &Window, roamed: bool) {
        self.rekeys += window.rekeys.len() as u32;
        self.reconnects += window.connects;
        self.roams += u32::from(roamed);
        if let Some(frames) = window.frames {
            self.multicast += frames.multicast;
            self.unicast += frames.unicast;
        }
        if let Some(delta) = window.firmware {
            let total = self.firmware.get_or_insert_default();
            total.group_frames += delta.group_frames;
            total.beacons_missed += delta.beacons_missed;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Link {
    bssid: String,
    since: f64,
    /// End of the last window with a broadcast frame, or the association.
    quiet_since: f64,
    quiet: Tally,
    suspect_after: f64,
    calibration: Calibration,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Episode {
    /// The last broadcast frame before the silence (boot clock).
    pub started: f64,
    /// When the probe confirmed it.
    pub confirmed: f64,
    pub bssid: String,
    pub verdict: Verdict,
    pub calibration: Calibration,
    pub probe: ProbeResult,
    /// Seconds from the last group rekey to the start of the silence.
    pub rekey_before_start: Option<f64>,
    pub tally: Tally,
    /// The ongoing report went out; the end is not reported again.
    pub reported: bool,
    pub wall_offset: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndedBy {
    /// Broadcasts came back within a minute of a group rekey.
    Rekey,
    /// ... on another AP.
    Roam,
    /// ... after a new association.
    Reconnect,
    /// ... with none of those.
    Spontaneous,
    /// The link went down.
    Disconnected,
    /// Still silent after `report_ongoing_after`.
    StillOngoing,
    /// The machine rebooted during the episode.
    Reboot,
    /// The monitor was turned off.
    Stopped,
}

impl EndedBy {
    pub fn code(self) -> &'static str {
        match self {
            EndedBy::Rekey => "rekey",
            EndedBy::Roam => "roam",
            EndedBy::Reconnect => "reconnect",
            EndedBy::Spontaneous => "spontaneous",
            EndedBy::Disconnected => "disconnected",
            EndedBy::StillOngoing => "still_ongoing",
            EndedBy::Reboot => "reboot",
            EndedBy::Stopped => "stopped",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ended {
    pub episode: Episode,
    /// Boot clock of the episode's boot.
    pub end: f64,
    pub ended_by: EndedBy,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Learn how the DHCP server answers, while broadcasts arrive.
    Calibrate,
    /// Test group reception now.
    Probe,
    /// Report this episode.
    Report(Box<Ended>),
}

/// Wall-clock records for the 24 h counters, kept across reboots.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct History {
    rekeys: VecDeque<f64>,
    episodes: VecDeque<f64>,
    /// (start, end) of silences while associated.
    silences: VecDeque<(f64, f64)>,
    /// (start, end) of the time the sniffer covered.
    covered: VecDeque<(f64, f64)>,
    probes: VecDeque<f64>,
    false_alarms: VecDeque<f64>,
}

impl History {
    fn prune(&mut self, now: f64) {
        let since = now - DAY;
        for list in [
            &mut self.rekeys,
            &mut self.episodes,
            &mut self.probes,
            &mut self.false_alarms,
        ] {
            while list.front().is_some_and(|t| *t < since) {
                list.pop_front();
            }
        }
        for list in [&mut self.silences, &mut self.covered] {
            while list.front().is_some_and(|(_, end)| *end < since) {
                list.pop_front();
            }
        }
    }

    fn add_rekey(&mut self, at: f64) {
        // The journal is read with a margin: the same line can come twice.
        if self.rekeys.iter().rev().any(|t| (t - at).abs() < 1e-3) {
            return;
        }
        let position = self.rekeys.partition_point(|t| *t < at);
        self.rekeys.insert(position, at);
    }

    fn cover(&mut self, start: f64, end: f64) {
        if let Some(last) = self.covered.back_mut()
            && start - last.1 <= 1.0
        {
            last.1 = end;
            return;
        }
        self.covered.push_back((start, end));
    }

    fn last_rekey_before(&self, at: f64) -> Option<f64> {
        self.rekeys.iter().rev().find(|t| **t <= at).copied()
    }

    /// The median gap between consecutive rekeys.
    fn rekey_interval(&self) -> Option<f64> {
        let mut gaps: Vec<f64> = self
            .rekeys
            .iter()
            .zip(self.rekeys.iter().skip(1))
            .map(|(a, b)| b - a)
            .filter(|gap| *gap > 0.0 && *gap <= MAX_REKEY_INTERVAL)
            .collect();
        if gaps.is_empty() {
            return None;
        }
        gaps.sort_by(f64::total_cmp);
        Some(gaps[gaps.len() / 2])
    }
}

/// The denominators published with the device attributes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Counters {
    /// Seconds of the last 24 h the sniffer covered while associated.
    pub covered_s: u64,
    pub rekeys: u64,
    pub episodes: u64,
    pub longest_silence_s: u64,
    pub probes: u64,
    /// Probes that found group reception working after a silence.
    pub false_alarms: u64,
    pub rekey_interval_s: Option<u64>,
}

/// Everything the detector keeps, as saved between runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Saved {
    link: Option<Link>,
    episode: Option<Episode>,
    last_probe: Option<f64>,
    pending: Option<Pending>,
    wall_offset: f64,
    /// Boot clock of the last window.
    last_window: f64,
    history: History,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum Pending {
    Calibration,
    Probe,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Detector {
    thresholds: Thresholds,
    state: Saved,
}

impl Detector {
    pub fn new(thresholds: Thresholds) -> Detector {
        Detector {
            thresholds,
            state: Saved::default(),
        }
    }

    /// Resume from what an earlier run saved. Within the same boot the
    /// link and an open episode carry on; after a reboot the episode is
    /// over, and the returned report says so.
    pub fn restore(
        thresholds: Thresholds,
        saved: Saved,
        same_boot: bool,
    ) -> (Detector, Option<Ended>) {
        let mut state = saved;
        state.pending = None;
        let mut ended = None;
        if !same_boot {
            state.link = None;
            state.last_probe = None;
            if let Some(episode) = state.episode.take()
                && !episode.reported
            {
                ended = Some(Ended {
                    end: state.last_window,
                    ended_by: EndedBy::Reboot,
                    episode,
                });
            }
        }
        (Detector { thresholds, state }, ended)
    }

    pub fn saved(&self) -> &Saved {
        &self.state
    }

    pub fn episode(&self) -> Option<&Episode> {
        self.state.episode.as_ref()
    }

    pub fn calibration(&self) -> Calibration {
        self.state
            .link
            .as_ref()
            .map_or(Calibration::Pending, |link| link.calibration)
    }

    /// Seconds since the last broadcast frame while associated.
    pub fn quiet_for(&self, now: f64) -> Option<f64> {
        self.state.link.as_ref().map(|link| now - link.quiet_since)
    }

    pub fn observe(&mut self, window: &Window) -> Vec<Step> {
        let thresholds = self.thresholds;
        let state = &mut self.state;
        let offset = window.wall_offset;
        let now_wall = window.end + offset;
        state.wall_offset = offset;
        state.last_window = window.end;
        state.history.prune(now_wall);
        for rekey in &window.rekeys {
            state.history.add_rekey(rekey + offset);
        }
        let mut steps = Vec::new();

        let Some(bssid) = &window.bssid else {
            if let Some(link) = state.link.take() {
                close_silence(&mut state.history, &link, offset, window.end - WINDOW);
            }
            if let Some(episode) = state.episode.take() {
                steps.extend(report(episode, window.end, EndedBy::Disconnected));
            }
            return steps;
        };
        let link = state.link.get_or_insert_with(|| Link {
            bssid: bssid.clone(),
            since: window.end,
            quiet_since: window.end,
            quiet: Tally::default(),
            suspect_after: thresholds.suspect_after,
            calibration: Calibration::Pending,
        });
        let roamed = link.bssid != *bssid;
        link.bssid.clone_from(bssid);

        let Some(frames) = window.frames else {
            // The sniffer missed part of the window: no claim either way. An
            // open episode stays open until broadcasts are seen again.
            if let Some(episode) = &mut state.episode {
                link.quiet.add(window, roamed);
                episode.tally = link.quiet;
            } else {
                close_silence(&mut state.history, link, offset, window.end - WINDOW);
                link.quiet_since = window.end;
                link.quiet = Tally::default();
            }
            return steps;
        };
        state.history.cover(now_wall - WINDOW, now_wall);
        link.quiet.add(window, roamed);

        if frames.broadcast > 0 {
            close_silence(&mut state.history, link, offset, window.end - WINDOW);
            if let Some(mut episode) = state.episode.take() {
                episode.tally = link.quiet;
                let healed_by_rekey = state
                    .history
                    .last_rekey_before(now_wall)
                    .is_some_and(|t| now_wall - t <= HEALED_BY_REKEY_WITHIN);
                let ended_by = if roamed {
                    EndedBy::Roam
                } else if window.connects > 0 {
                    EndedBy::Reconnect
                } else if healed_by_rekey {
                    EndedBy::Rekey
                } else {
                    EndedBy::Spontaneous
                };
                steps.extend(report(episode, window.end, ended_by));
                link.suspect_after = thresholds.suspect_after;
            }
            link.quiet_since = window.end;
            link.quiet = Tally::default();
            if link.calibration == Calibration::Pending
                && window.has_ipv4
                && window.end - link.since >= thresholds.calibrate_after
                && state.pending.is_none()
                && probe_allowed(state.last_probe, window.end, &thresholds)
            {
                state.pending = Some(Pending::Calibration);
                state.last_probe = Some(window.end);
                steps.push(Step::Calibrate);
            }
            return steps;
        }

        if let Some(episode) = &mut state.episode {
            episode.tally = link.quiet;
            if !episode.reported && window.end - episode.started >= thresholds.report_ongoing_after
            {
                episode.reported = true;
                steps.push(Step::Report(Box::new(Ended {
                    episode: episode.clone(),
                    end: window.end,
                    ended_by: EndedBy::StillOngoing,
                })));
            }
            return steps;
        }
        // Without unicast either the link is dead or idle: that is the
        // Wi-Fi health check's business, not this signature.
        if window.has_ipv4
            && link.quiet.unicast > 0
            && window.end - link.quiet_since >= link.suspect_after
            && state.pending.is_none()
            && probe_allowed(state.last_probe, window.end, &thresholds)
        {
            state.pending = Some(Pending::Probe);
            state.last_probe = Some(window.end);
            steps.push(Step::Probe);
        }
        steps
    }

    /// The result of the probe or calibration the last window asked for.
    pub fn probed(&mut self, result: ProbeResult) {
        let state = &mut self.state;
        let Some(pending) = state.pending.take() else {
            return;
        };
        let Some(link) = &mut state.link else { return };
        let now_wall = state.last_window + state.wall_offset;
        state.history.probes.push_back(now_wall);
        if pending == Pending::Calibration {
            if let Some(calibration) = probe::calibration(&result) {
                link.calibration = calibration;
            }
            return;
        }
        let verdict = probe::classify(&result);
        if verdict == Verdict::GroupRxWorks {
            state.history.false_alarms.push_back(now_wall);
            link.suspect_after = (link.suspect_after * 2.0).min(self.thresholds.max_suspect_after);
            return;
        }
        if state.episode.is_some() {
            return;
        }
        let started_wall = link.quiet_since + state.wall_offset;
        state.history.episodes.push_back(started_wall);
        state.episode = Some(Episode {
            started: link.quiet_since,
            confirmed: state.last_window,
            bssid: link.bssid.clone(),
            verdict,
            calibration: link.calibration,
            probe: result,
            rekey_before_start: state
                .history
                .last_rekey_before(started_wall)
                .map(|t| started_wall - t),
            tally: link.quiet,
            reported: false,
            wall_offset: state.wall_offset,
        });
    }

    /// The monitor is being turned off: an open episode ends here.
    pub fn stop(&mut self, now: f64) -> Option<Ended> {
        let state = &mut self.state;
        state.pending = None;
        if let Some(link) = state.link.take() {
            close_silence(&mut state.history, &link, state.wall_offset, now);
        }
        let episode = state.episode.take()?;
        (!episode.reported).then_some(Ended {
            episode,
            end: now,
            ended_by: EndedBy::Stopped,
        })
    }

    /// Rekeys from before the monitor started (the journal of the last
    /// 24 h), for the counters and the interval estimate only.
    pub fn backfill_rekeys(&mut self, walls: &[f64]) {
        for wall in walls {
            self.state.history.add_rekey(*wall);
        }
    }

    /// The last 24 h, counting the silence still running.
    pub fn counters(&self, now: f64, wall_offset: f64) -> Counters {
        let state = &self.state;
        let now_wall = now + wall_offset;
        let since = now_wall - DAY;
        let span = |(start, end): &(f64, f64)| (end.min(now_wall) - start.max(since)).max(0.0);
        let current = state
            .link
            .as_ref()
            .map_or(0.0, |link| now - link.quiet_since);
        let count = |list: &VecDeque<f64>| list.iter().filter(|t| **t >= since).count() as u64;
        Counters {
            covered_s: state.history.covered.iter().map(span).sum::<f64>() as u64,
            rekeys: count(&state.history.rekeys),
            episodes: count(&state.history.episodes),
            longest_silence_s: state
                .history
                .silences
                .iter()
                .map(span)
                .fold(current.max(0.0), f64::max) as u64,
            probes: count(&state.history.probes),
            false_alarms: count(&state.history.false_alarms),
            rekey_interval_s: state.history.rekey_interval().map(|s| s as u64),
        }
    }
}

fn probe_allowed(last: Option<f64>, now: f64, thresholds: &Thresholds) -> bool {
    last.is_none_or(|last| now - last >= thresholds.probe_interval)
}

/// Record the silence that ends at `end` (boot clock), if any.
fn close_silence(history: &mut History, link: &Link, offset: f64, end: f64) {
    if end > link.quiet_since {
        history
            .silences
            .push_back((link.quiet_since + offset, end + offset));
    }
}

fn report(episode: Episode, end: f64, ended_by: EndedBy) -> Option<Step> {
    (!episode.reported).then(|| {
        Step::Report(Box::new(Ended {
            episode,
            end,
            ended_by,
        }))
    })
}
