//! `manual_mode.py`: a regular profile made from the targets of a recorded
//! manual brew. The building is the `profile` crate's; this finds the brew.

use chrono::{Datelike, Timelike};
use serde_json::Value;

use crate::app::App;
use crate::profiles::ProfileError;

#[derive(Debug, thiserror::Error)]
pub enum FromManualError {
    /// `ManualShotNotFound`.
    #[error("{0}")]
    NotFound(String),
    /// `ManualShotHasNoTargets`.
    #[error("the manual brew recorded no pressure or flow targets")]
    NoTargets,
    /// The new profile was refused.
    #[error("{0}")]
    Profile(ProfileError),
    #[error("{0}")]
    Other(String),
}

impl From<profile::ManualShotError> for FromManualError {
    fn from(error: profile::ManualShotError) -> FromManualError {
        match error {
            profile::ManualShotError::NotFound(message) => FromManualError::NotFound(message),
            profile::ManualShotError::NoTargets => FromManualError::NoTargets,
            other => FromManualError::Other(other.to_string()),
        }
    }
}

/// `find_manual_shot(shot_id)`: the requested brew, or the latest brewed with
/// the Manual mode profile.
pub fn find_manual_shot(app: &App, shot_id: Option<&str>) -> Result<Value, FromManualError> {
    let ids = profile::manual_shot_search_ids(shot_id);
    let results = app
        .history
        .search_manual_shots(&ids)
        .map_err(FromManualError::Other)?;
    Ok(profile::pick_manual_shot(shot_id, results)?)
}

struct Env;

impl profile::ManualShotEnv for Env {
    fn new_uuid(&mut self) -> String {
        uuid::Uuid::new_v4().to_string()
    }

    fn local_time(&self, timestamp: f64) -> Result<profile::LocalDateTime, (String, String)> {
        if !timestamp.is_finite() {
            return Err((
                "ValueError".into(),
                "cannot convert float NaN to integer".into(),
            ));
        }
        let t = crate::shots::local_time(timestamp);
        Ok(profile::LocalDateTime {
            year: t.year(),
            month: t.month(),
            day: t.day(),
            hour: t.hour(),
            minute: t.minute(),
        })
    }
}

/// `build_profile_from_manual_shot(shot, name)`.
pub fn build_profile_from_manual_shot(
    shot: &Value,
    name: Option<&str>,
) -> Result<Value, FromManualError> {
    Ok(profile::build_profile_from_manual_shot(
        shot, name, &mut Env,
    )?)
}
