//! `pour_over_profiles.py`: Pour Over recipes, one validated JSON file each.
//! Validation lives in the `profile` crate; this is the storage.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

use parking_lot::Mutex;
use serde_json::{Value, json};

pub use profile::pour_over::MAX_POUR_OVER_PROFILE_BYTES;
use profile::pour_over::{
    MAX_POUR_OVER_PROFILES, MIN_FREE_PROFILE_BYTES, PourOverProfileError, pour_over_profile_image,
    pour_over_summary, serialize_pour_over_profile, validate_pour_over_profile,
};

use crate::app::App;
use crate::http::Reply;
use crate::profiles::ProfileEvent;

/// A request body over the size limit that names itself a Pour Over profile:
/// refused before it is parsed.
pub fn looks_like_pour_over(body: &[u8]) -> bool {
    profile::pour_over::is_oversized_pour_over_body(body)
}

/// `write_pour_over_profile_error`.
pub fn error_reply(error: &PourOverProfileError) -> Reply {
    let (status, body) = error.http_response();
    Reply::base().status(status).json(body)
}

#[derive(Default)]
struct State {
    /// Summaries (no image) by id.
    known: BTreeMap<String, Value>,
    available: bool,
}

pub struct PourOverProfileManager {
    dir: PathBuf,
    state: Mutex<State>,
}

/// `json.load(..., parse_constant=raise)`: strict JSON.
fn read_profile_file(path: &std::path::Path) -> Result<Value, PourOverProfileError> {
    let meta = std::fs::metadata(path).map_err(|e| PourOverProfileError::Storage(e.to_string()))?;
    if meta.len() > MAX_POUR_OVER_PROFILE_BYTES as u64 {
        return Err(PourOverProfileError::TooLarge);
    }
    let text =
        std::fs::read_to_string(path).map_err(|e| PourOverProfileError::Storage(e.to_string()))?;
    serde_json::from_str(&text).map_err(|e| PourOverProfileError::Other(e.to_string()))
}

impl PourOverProfileManager {
    pub fn new(dir: PathBuf) -> PourOverProfileManager {
        PourOverProfileManager {
            dir,
            state: Mutex::new(State::default()),
        }
    }

    /// `init`: Pour Over is optional; a failure leaves it unavailable and
    /// espresso unaffected.
    pub fn init(&self) {
        match self.refresh() {
            Ok(()) => self.state.lock().available = true,
            Err(error) => {
                let mut state = self.state.lock();
                state.known.clear();
                state.available = false;
                tracing::error!("Pour Over profiles are unavailable: {error}");
            }
        }
    }

    pub fn is_available(&self) -> bool {
        self.state.lock().available
    }

    fn require_available(&self) -> Result<(), PourOverProfileError> {
        if self.is_available() {
            Ok(())
        } else {
            Err(PourOverProfileError::Unavailable(
                "Pour Over profiles are unavailable".into(),
            ))
        }
    }

    fn ensure_directory(&self) -> std::io::Result<()> {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o755)
            .create(&self.dir)
    }

    /// `_refresh_profile_list`.
    fn refresh(&self) -> std::io::Result<()> {
        self.ensure_directory()?;
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&self.dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        entries.sort();
        for path in &entries {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if name.starts_with('.')
                && name.ends_with(".tmp")
                && let Err(error) = std::fs::remove_file(path)
            {
                tracing::warn!(
                    "Could not remove stale Pour Over temporary file {name}: {}",
                    error.kind()
                );
            }
        }
        let mut valid = BTreeMap::new();
        for path in entries
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e == "json"))
        {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            let outcome = read_profile_file(path)
                .and_then(|p| validate_pour_over_profile(&p))
                .and_then(|profile| {
                    let id = profile
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    if path.file_stem().and_then(|s| s.to_str()) != Some(id.as_str()) {
                        return Err(PourOverProfileError::Validation(Vec::new()));
                    }
                    Ok((id, pour_over_summary(&profile)))
                });
            match outcome {
                Ok((id, summary)) => {
                    valid.insert(id, summary);
                }
                Err(error) => tracing::warn!(
                    "Skipping invalid stored Pour Over profile {name}: {}",
                    error.class_name()
                ),
            }
        }
        self.state.lock().known = valid;
        Ok(())
    }

    /// `list_profiles`: by name (case-folded), then id.
    pub fn list_profiles(&self) -> Result<Vec<Value>, PourOverProfileError> {
        self.require_available()?;
        let mut profiles: Vec<Value> = self.state.lock().known.values().cloned().collect();
        profiles.sort_by_key(|p| {
            (
                p.get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_lowercase(),
                p.get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        });
        Ok(profiles)
    }

    /// `get_profile`: the summary, or the stored file read and validated again.
    pub fn get_profile(
        &self,
        id: &str,
        include_image: bool,
    ) -> Result<Option<Value>, PourOverProfileError> {
        self.require_available()?;
        let Some(summary) = self.state.lock().known.get(id).cloned() else {
            return Ok(None);
        };
        if !include_image {
            return Ok(Some(summary));
        }
        let path = self.dir.join(format!("{id}.json"));
        let stored = read_profile_file(&path)
            .and_then(|p| validate_pour_over_profile(&p))
            .and_then(|p| {
                if p.get("id").and_then(Value::as_str) != Some(id) {
                    Err(PourOverProfileError::Validation(Vec::new()))
                } else {
                    Ok(p)
                }
            });
        match stored {
            Ok(profile) => Ok(Some(profile)),
            Err(error) => {
                tracing::warn!(
                    "Could not read stored Pour Over profile {id}: {}",
                    error.class_name()
                );
                Err(PourOverProfileError::Storage(
                    "Could not read profile".into(),
                ))
            }
        }
    }

    /// `get_profile_image`: the profile's JPEG.
    pub fn get_profile_image(&self, id: &str) -> Result<Option<Vec<u8>>, PourOverProfileError> {
        let Some(profile) = self.get_profile(id, true)? else {
            return Ok(None);
        };
        pour_over_profile_image(&profile)
    }

    /// `save_profile`: validated, size and count limits, free space, an
    /// atomic write, and the `profile` event marked as Pour Over.
    pub fn save_profile(
        &self,
        app: &App,
        value: Value,
        change_id: Option<String>,
    ) -> Result<Value, PourOverProfileError> {
        self.require_available()?;
        let profile = validate_pour_over_profile(&value)?;
        let serialized = serialize_pour_over_profile(&profile)?;
        let id = profile
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.ensure_directory()
            .map_err(|e| PourOverProfileError::Storage(e.to_string()))?;
        let is_update = self.state.lock().known.contains_key(&id);
        if !is_update && self.state.lock().known.len() >= MAX_POUR_OVER_PROFILES {
            return Err(PourOverProfileError::Limit);
        }
        let free = nix::sys::statvfs::statvfs(&self.dir)
            .map(|s| s.blocks_available() * s.fragment_size())
            .unwrap_or(u64::MAX);
        if free < MIN_FREE_PROFILE_BYTES + serialized.len() as u64 {
            return Err(PourOverProfileError::Storage(
                "Not enough free storage".into(),
            ));
        }
        self.write_atomically(&id, serialized.as_bytes())
            .map_err(|_| PourOverProfileError::Storage("Could not save profile".into()))?;
        self.state
            .lock()
            .known
            .insert(id.clone(), pour_over_summary(&profile));
        let change_id = change_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let event = if is_update {
            ProfileEvent::Update
        } else {
            ProfileEvent::Create
        };
        app.profiles
            .emit_event(event, Some(&id), Some(&change_id), Some("pour_over"));
        tracing::info!("Saved Pour Over profile {id}");
        Ok(json!({"profile": profile, "change_id": change_id}))
    }

    fn write_atomically(&self, id: &str, bytes: &[u8]) -> std::io::Result<()> {
        let mut temporary = tempfile::Builder::new()
            .prefix(&format!(".{id}."))
            .suffix(".tmp")
            .tempfile_in(&self.dir)?;
        temporary.write_all(bytes)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(self.dir.join(format!("{id}.json")))
            .map_err(|e| e.error)?;
        std::fs::File::open(&self.dir)?.sync_all()
    }

    /// `delete_profile`.
    pub fn delete_profile(
        &self,
        app: &App,
        id: &str,
        change_id: Option<String>,
    ) -> Result<Option<Value>, PourOverProfileError> {
        self.require_available()?;
        let Some(profile) = self.state.lock().known.get(id).cloned() else {
            return Ok(None);
        };
        match std::fs::remove_file(self.dir.join(format!("{id}.json"))) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(PourOverProfileError::Storage(
                    "Could not delete profile".into(),
                ));
            }
        }
        self.state.lock().known.remove(id);
        if let Err(error) = std::fs::File::open(&self.dir).and_then(|d| d.sync_all()) {
            tracing::warn!(
                "Could not fsync Pour Over profile deletion for {id}: {}",
                error.kind()
            );
        }
        let change_id = change_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        app.profiles.emit_event(
            ProfileEvent::Delete,
            Some(id),
            Some(&change_id),
            Some("pour_over"),
        );
        tracing::info!("Deleted Pour Over profile {id}");
        Ok(Some(json!({"profile": profile, "change_id": change_id})))
    }

    /// The schema as `json.dumps` writes it.
    pub fn schema_text(&self) -> Result<String, PourOverProfileError> {
        self.require_available()?;
        let schema: Value = serde_json::from_str(profile::pour_over::POUR_OVER_PROFILE_SCHEMA)
            .map_err(|e| PourOverProfileError::Other(e.to_string()))?;
        Ok(pyfmt::dumps_default(&schema))
    }
}
