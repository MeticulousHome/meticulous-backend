//! `simple_profile.py`: the bundled profile limited access serves and new
//! profiles start from.

use std::borrow::Cow;
use std::path::PathBuf;

use parking_lot::Mutex;
use serde_json::{Value, json};

pub struct SimpleProfile {
    path: PathBuf,
    profile: Mutex<Option<Value>>,
}

impl SimpleProfile {
    pub fn new(path: PathBuf) -> SimpleProfile {
        SimpleProfile {
            path,
            profile: Mutex::new(None),
        }
    }

    /// `SimpleProfile.load`: the file, or the built-in profile when there is
    /// no file; `None` (logged at ERROR) when the file cannot be read, is not
    /// JSON, or is not a profile with an id.
    ///
    /// Deliberate change: Python answers `None` for a missing file too
    /// (tests/test_simple_profile.py::test_missing_file_is_empty_not_fatal),
    /// which leaves a factory machine without its only profile.
    pub fn load(&self) -> Option<Value> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => Ok(Cow::Owned(text)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tracing::warn!(
                    "No simple profile at {}, using the built-in one",
                    self.path.display()
                );
                Ok(Cow::Borrowed(crate::bundled::SIMPLE_PROFILE))
            }
            Err(error) => Err(error),
        };
        let loaded = match text.map(|t| serde_json::from_str::<Value>(&t)) {
            Err(error) => {
                tracing::error!(
                    "Could not load simple profile from {}: {error}",
                    self.path.display()
                );
                None
            }
            Ok(Err(error)) => {
                tracing::error!(
                    "Could not load simple profile from {}: {error}",
                    self.path.display()
                );
                None
            }
            Ok(Ok(profile)) if profile.get("id").is_some_and(|id| id.is_string()) => {
                // Bound first: inside tracing's macros `Value` names its trait.
                let id = profile["id"].as_str().unwrap_or_default().to_string();
                let name = profile
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("None")
                    .to_string();
                tracing::info!("Loaded simple profile {id} ({name})");
                Some(profile)
            }
            Ok(Ok(_)) => {
                tracing::error!("Simple profile is not a profile object with an id");
                None
            }
        };
        *self.profile.lock() = loaded.clone();
        loaded
    }

    pub fn get(&self) -> Option<Value> {
        let cached = self.profile.lock().clone();
        cached.or_else(|| self.load())
    }

    pub fn get_by_id(&self, id: &str) -> Option<Value> {
        self.get()
            .filter(|p| p.get("id").and_then(Value::as_str) == Some(id))
    }

    /// `SimpleProfile.list(full)`: the profile, without stages unless full.
    pub fn list(&self, full: bool) -> Vec<Value> {
        let Some(mut profile) = self.get() else {
            return Vec::new();
        };
        if !full && let Some(map) = profile.as_object_mut() {
            map.shift_remove("stages");
        }
        vec![profile]
    }

    /// `SimpleProfile.defaults()`.
    pub fn defaults(&self) -> Value {
        let default: Vec<Value> = self.get().into_iter().collect();
        json!({"default": default, "community": []})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tests/test_simple_profile.py: the bundled profile loads and lists.
    #[test]
    fn bundled_profile() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../simple_profile.json");
        let simple = SimpleProfile::new(path);
        let profile = simple.get().expect("bundled simple profile");
        assert_eq!(profile["id"], "f5db85cc-ab0d-4c81-aee8-bcac7da77141");
        assert!(simple.list(false)[0].get("stages").is_none());
        assert!(simple.list(true)[0].get("stages").is_some());
        assert_eq!(simple.defaults()["community"], json!([]));
        assert!(simple.get_by_id("nope").is_none());
    }

    /// tests/test_simple_profile.py::test_path_override: a file that is
    /// there wins over the built-in profile.
    #[test]
    fn file_overrides_the_built_in_profile() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let custom = dir.path().join("custom.json");
        std::fs::write(
            &custom,
            r#"{"id": "custom", "name": "Custom", "stages": []}"#,
        )
        .expect("custom profile");
        assert_eq!(
            SimpleProfile::new(custom).get().expect("custom")["id"],
            "custom"
        );
    }

    /// A file that is there but broken is an override gone wrong: reported,
    /// not replaced by the built-in profile.
    #[test]
    fn broken_file_is_none() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let broken = dir.path().join("simple.json");
        std::fs::write(&broken, "{not json").expect("broken profile");
        let simple = SimpleProfile::new(broken);
        assert!(simple.get().is_none());
        assert_eq!(simple.defaults()["default"], json!([]));
    }
}
