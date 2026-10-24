//! Report progress, contract v1 (`ReportProgress`, `_report_progress`,
//! `_progress_entries` and `_progress_for_row` in `api/bug_report.py`): one
//! `report_progress` event per change, broadcast to the Dial and the mobile
//! app, and `GET /reports/progress` for clients that connect mid-report.
//!
//! In memory on purpose: progress describes reports in flight, and a restart
//! ends every collection anyway. Queued rows are rebuilt from the database.
//! Ended reports stay (the list hides them after a minute): a queued row the
//! Dial gave up on keeps its status, and must not come back as `queued`.

use std::collections::HashMap;

use history::bug_reports::BugReportRow;
use serde_json::{Value, json};

use super::{REPORT_STATUS_CANCELLED, REPORT_STATUS_QUEUED};

pub const REPORT_PROGRESS_EVENT: &str = "report_progress";
pub const REPORT_PROGRESS_VERSION: i64 = 1;
pub const REPORT_PROGRESS_FINAL_RETENTION_SECONDS: i64 = 60;

/// `REPORT_STAGES` in their order, then `REPORT_FINAL_STAGES`. The derived
/// order is the bar's, and only compares stages that are not final.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Queued,
    Preparing,
    FetchingMachineInfo,
    FetchingLogs,
    FetchingMachineStatus,
    CollectingDebugShots,
    Ticketing,
    Compressing,
    Uploading,
    Finishing,
    Submitted,
    Failed,
    Cancelled,
}

impl Stage {
    /// `REPORT_STAGES`.
    pub const ORDER: [Stage; 10] = [
        Stage::Queued,
        Stage::Preparing,
        Stage::FetchingMachineInfo,
        Stage::FetchingLogs,
        Stage::FetchingMachineStatus,
        Stage::CollectingDebugShots,
        Stage::Ticketing,
        Stage::Compressing,
        Stage::Uploading,
        Stage::Finishing,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Stage::Queued => "queued",
            Stage::Preparing => "preparing",
            Stage::FetchingMachineInfo => "fetching_machine_info",
            Stage::FetchingLogs => "fetching_logs",
            Stage::FetchingMachineStatus => "fetching_machine_status",
            Stage::CollectingDebugShots => "collecting_debug_shots",
            Stage::Ticketing => "ticketing",
            Stage::Compressing => "compressing",
            Stage::Uploading => "uploading",
            Stage::Finishing => "finishing",
            Stage::Submitted => "submitted",
            Stage::Failed => "failed",
            Stage::Cancelled => "cancelled",
        }
    }

    /// `stage in REPORT_FINAL_STAGES`.
    pub fn is_final(self) -> bool {
        matches!(self, Stage::Submitted | Stage::Failed | Stage::Cancelled)
    }

    /// `REPORT_UNCANCELLABLE_STAGES`: once the archive is on its way to
    /// Sentry the feedback cannot be recalled.
    fn uncancellable(self) -> bool {
        matches!(self, Stage::Uploading | Stage::Finishing)
    }

    /// `REPORT_UPLOADER_STAGES`: the stages only the uploader (the Dial)
    /// knows about, reported through POST /reports/<localID>/progress.
    pub const UPLOADER: [Stage; 2] = [Stage::Ticketing, Stage::Uploading];

    /// The uploader stage named `name`.
    pub fn uploader(name: &str) -> Option<Stage> {
        Stage::UPLOADER
            .into_iter()
            .find(|stage| stage.as_str() == name)
    }
}

/// Where a report was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Dial,
    Mobile,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Dial => "dial",
            Origin::Mobile => "mobile",
        }
    }
}

/// `detail` during `collecting_debug_shots`: debug shots copied so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageDetail {
    pub current: usize,
    pub total: usize,
}

/// `failure` of a `failed` report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub code: String,
    pub message: String,
}

/// `ReportProgress`: where one report stands, as the Dial and the mobile app
/// draw it. `ticket` is `null` without one; the times are the row's values
/// for an entry rebuilt from the database.
#[derive(Debug, Clone, PartialEq)]
pub struct ReportProgress {
    pub local_id: String,
    pub origin: Origin,
    pub ticket: Value,
    pub stage: Stage,
    pub started_at: Value,
    pub updated_at: Value,
    pub detail: Option<StageDetail>,
    pub failure: Option<Failure>,
}

impl ReportProgress {
    pub fn new(
        local_id: &str,
        origin: Origin,
        ticket: Value,
        stage: Stage,
        started_at: Value,
        updated_at: Value,
    ) -> ReportProgress {
        ReportProgress {
            local_id: local_id.to_string(),
            origin,
            ticket,
            stage,
            started_at,
            updated_at,
            detail: None,
            failure: None,
        }
    }

    pub fn is_final(&self) -> bool {
        self.stage.is_final()
    }

    pub fn cancellable(&self) -> bool {
        !self.is_final() && !self.stage.uncancellable()
    }

    /// The stages of this report: the Dial asks for the ticket of its own
    /// reports, a mobile report comes with one and starts `queued`.
    pub fn stages(&self) -> Vec<&'static str> {
        let skipped = match self.origin {
            Origin::Mobile => Stage::Ticketing,
            Origin::Dial => Stage::Queued,
        };
        Stage::ORDER
            .iter()
            .filter(|stage| **stage != skipped)
            .map(|stage| stage.as_str())
            .collect()
    }

    pub fn payload(&self) -> Value {
        pyfmt::object([
            ("v", json!(REPORT_PROGRESS_VERSION)),
            ("localID", json!(self.local_id)),
            ("origin", json!(self.origin.as_str())),
            ("ticket", self.ticket.clone()),
            ("stage", json!(self.stage.as_str())),
            ("stages", json!(self.stages())),
            (
                "detail",
                self.detail.map_or(Value::Null, |d| {
                    pyfmt::object([("current", json!(d.current)), ("total", json!(d.total))])
                }),
            ),
            ("cancellable", json!(self.cancellable())),
            ("final", json!(self.is_final())),
            (
                "failure",
                self.failure.as_ref().map_or(Value::Null, |f| {
                    pyfmt::object([("code", json!(f.code)), ("message", json!(f.message))])
                }),
            ),
            ("startedAt", self.started_at.clone()),
            ("updatedAt", self.updated_at.clone()),
        ])
    }
}

/// `_report_progress`'s keyword arguments.
#[derive(Debug, Clone, Default)]
pub struct StageUpdate {
    /// Creates the entry of a report that has none; without it such a report
    /// is not tracked.
    pub origin: Option<Origin>,
    /// `Value::Null` keeps the ticket the entry has.
    pub ticket: Value,
    pub detail: Option<StageDetail>,
    pub failure: Option<Failure>,
    /// Lets a dispatched report whose collection stopped go back to `queued`.
    pub rewind: bool,
}

/// `_progress`: every report this process has tracked, by localID.
#[derive(Debug, Default)]
pub struct ProgressBook {
    entries: HashMap<String, ReportProgress>,
}

impl ProgressBook {
    pub fn get(&self, local_id: &str) -> Option<&ReportProgress> {
        self.entries.get(local_id)
    }

    /// `_report_progress` without the emit: the entry and whether it
    /// changed. Stages only move forward and a final stage is never left, so
    /// a retried request (a second GET of the draft, a repeated `uploading`)
    /// cannot drag the bar back; `rewind` is the one exception.
    pub fn advance(
        &mut self,
        local_id: &str,
        stage: Stage,
        update: StageUpdate,
        now: i64,
    ) -> Option<(ReportProgress, bool)> {
        let Some(entry) = self.entries.get_mut(local_id) else {
            let origin = update.origin?;
            let mut entry = ReportProgress::new(
                local_id,
                origin,
                update.ticket,
                stage,
                json!(now),
                json!(now),
            );
            entry.detail = update.detail;
            entry.failure = update.failure;
            self.entries.insert(local_id.to_string(), entry.clone());
            return Some((entry, true));
        };
        if entry.is_final() {
            return Some((entry.clone(), false));
        }
        if !stage.is_final() && !update.rewind {
            if stage < entry.stage {
                return Some((entry.clone(), false));
            }
            let same_ticket =
                update.ticket.is_null() || profile::pynum::py_eq(&update.ticket, &entry.ticket);
            if stage == entry.stage && update.detail == entry.detail && same_ticket {
                return Some((entry.clone(), false));
            }
        }
        entry.stage = stage;
        entry.detail = update.detail;
        entry.failure = update.failure;
        if !update.ticket.is_null() {
            entry.ticket = update.ticket;
        }
        entry.updated_at = json!(now);
        Some((entry.clone(), true))
    }

    /// `_progress_entries()` over the queued rows: every report in flight,
    /// the queued ones from the database included, and the ones that ended
    /// in the last minute, oldest first.
    pub fn listed(&mut self, queued: &[BugReportRow], now: i64) -> Vec<Value> {
        for row in queued {
            let Some(local_id) = row.local_id.as_str() else {
                continue;
            };
            self.entries.entry(local_id.to_string()).or_insert_with(|| {
                ReportProgress::new(
                    local_id,
                    Origin::Mobile,
                    row.ticket_number.clone(),
                    Stage::Queued,
                    row.creation_time.clone(),
                    row.creation_time.clone(),
                )
            });
        }
        let shown_since = now - REPORT_PROGRESS_FINAL_RETENTION_SECONDS;
        let mut entries: Vec<&ReportProgress> = self
            .entries
            .values()
            .filter(|entry| {
                !entry.is_final() || entry.updated_at.as_i64().is_some_and(|t| t >= shown_since)
            })
            .collect();
        // Deliberate fix: a row without a creation time sorts first, where
        // the Python's sort raises comparing None with an int.
        entries.sort_by(|a, b| {
            (a.started_at.as_i64(), &a.local_id).cmp(&(b.started_at.as_i64(), &b.local_id))
        });
        entries.into_iter().map(ReportProgress::payload).collect()
    }

    /// The `_progress_for_row` entry of a report this process has not
    /// tracked: what its row says, kept from now on.
    pub fn track_row(&mut self, local_id: &str, row: &BugReportRow, now: i64) -> ReportProgress {
        self.entries
            .entry(local_id.to_string())
            .or_insert_with(|| {
                let stage = match row.status_str() {
                    Some(REPORT_STATUS_QUEUED) => Stage::Queued,
                    Some("submitted") => Stage::Submitted,
                    Some(REPORT_STATUS_CANCELLED) => Stage::Cancelled,
                    _ => Stage::Compressing,
                };
                // A row only carries contact details when the mobile app
                // dispatched it.
                let contact = crate::config::python_truthy(&row.contact_name)
                    || crate::config::python_truthy(&row.contact_email);
                let origin = if contact || stage == Stage::Queued {
                    Origin::Mobile
                } else {
                    Origin::Dial
                };
                ReportProgress::new(
                    local_id,
                    origin,
                    row.ticket_number.clone(),
                    stage,
                    row.creation_time.clone(),
                    json!(now),
                )
            })
            .clone()
    }
}
