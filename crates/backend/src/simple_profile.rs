//! `simple_profile.py`: the bundled profile limited access serves and new
//! profiles start from.

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

    /// `SimpleProfile.load`: `None` (logged at ERROR) when the file is
    /// missing, not JSON, or not a profile with an id.
    pub fn load(&self) -> Option<Value> {
        let loaded =
            match std::fs::read_to_string(&self.path).map(|t| serde_json::from_str::<Value>(&t)) {
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
        if !full {
            if let Some(map) = profile.as_object_mut() {
                map.shift_remove("stages");
            }
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

    #[test]
    fn missing_file_is_none() {
        let simple = SimpleProfile::new(PathBuf::from("/nonexistent/simple.json"));
        assert!(simple.get().is_none());
        assert_eq!(simple.defaults()["default"], json!([]));
    }
}
