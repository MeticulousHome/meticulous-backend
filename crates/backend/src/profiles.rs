//! `profiles.py`: the espresso profiles on disk, their images, the change
//! log the Dial syncs from, the hovered profile, and loading one into the
//! machine.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use md5::{Digest, Md5};
use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use tstype::{Ts, field};

use crate::alarms::{AlarmType, minutes_left};
use crate::app::App;
use crate::config::{ConfigStore, keys};
use crate::images::WARNING_TRIANGLE_IMAGE;
use crate::notifications::now_seconds;
use crate::sio::Sio;

pub const DEFAULT_AUTHOR_ID: &str = "00000000-0000-0000-0000-000000000000";
const CHANGES_TO_KEEP: usize = 100;
const IMAGE_ROUTE: &str = "/api/v1/profile/image/";
const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
const IMAGE_EXTENSIONS: [&str; 6] = [".png", ".jpg", ".jpeg", ".gif", ".webm", ".webp"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileEvent {
    Create,
    Update,
    Delete,
    Reload,
    Load,
}

impl ProfileEvent {
    pub const ALL: [ProfileEvent; 5] = [
        ProfileEvent::Create,
        ProfileEvent::Update,
        ProfileEvent::Delete,
        ProfileEvent::Reload,
        ProfileEvent::Load,
    ];

    pub fn value(self) -> &'static str {
        match self {
            ProfileEvent::Create => "create",
            ProfileEvent::Update => "update",
            ProfileEvent::Delete => "delete",
            ProfileEvent::Reload => "full_reload",
            ProfileEvent::Load => "load",
        }
    }
}

/// The `brew_type` of the Pour Over profiles' events; espresso profiles'
/// events have none.
pub const POUR_OVER_BREW_TYPE: &str = "pour_over";

/// The payload of the `profile` event (`_emit_profile_event`).
pub fn profile_event_payload(
    change: ProfileEvent,
    profile_id: Option<&str>,
    change_id: Option<&str>,
    brew_type: Option<&str>,
) -> Value {
    let mut payload = Map::new();
    payload.insert("change".into(), json!(change.value()));
    if let Some(id) = profile_id {
        payload.insert("profile_id".into(), json!(id));
    }
    if let Some(id) = change_id {
        payload.insert("change_id".into(), json!(id));
    }
    if let Some(t) = brew_type {
        payload.insert("brew_type".into(), json!(t));
    }
    Value::Object(payload)
}

/// The TypeScript type of [`profile_event_payload`].
pub fn profile_event_type() -> Ts {
    Ts::named(
        "ProfilePayload",
        "The `profile` event: a profile was created, updated, deleted or loaded, or the whole\n\
         list changed (`full_reload`).",
        Ts::object([
            field(
                "change",
                Ts::named(
                    "ProfileChange",
                    "",
                    Ts::literals(ProfileEvent::ALL.map(ProfileEvent::value)),
                ),
            ),
            field("profile_id", Ts::String)
                .optional()
                .doc("Absent on `full_reload`."),
            field("change_id", Ts::String)
                .optional()
                .doc("The change log entry: on creates, updates and Pour Over deletes."),
            field("brew_type", Ts::literal(POUR_OVER_BREW_TYPE))
                .optional()
                .doc("Only on Pour Over profile events."),
        ]),
    )
}

/// `ProfileHover`: which profile a client has in focus. The values are kept
/// as a client sent them (`data.get("id", "")`), whatever their JSON type.
#[derive(Debug, Clone, PartialEq)]
pub struct Hover {
    pub id: Value,
    pub kind: Value,
    pub from: Value,
}

impl Default for Hover {
    fn default() -> Hover {
        Hover::new("", "", "")
    }
}

impl Hover {
    /// A hover the backend sets itself.
    pub fn new(id: &str, kind: &str, from: &str) -> Hover {
        Hover {
            id: json!(id),
            kind: json!(kind),
            from: json!(from),
        }
    }

    pub fn to_value(&self) -> Value {
        json!({"id": self.id, "type": self.kind, "from": self.from})
    }

    /// The TypeScript type of [`Hover::to_value`], the `profileHover` event.
    pub fn sio_type() -> Ts {
        Ts::named(
            "ProfileHoverPayload",
            "The `profileHover` event: the profile a client has in focus, passed on to the other\n\
             clients as that client sent it, whatever the JSON types (`from` is `backend` in the\n\
             hover a client gets on connect). Empty strings for keys the client left out, and\n\
             before any client sent one; the backend's own hovers are strings.",
            Ts::object([
                field("id", Ts::Unknown),
                field("type", Ts::Unknown),
                field("from", Ts::Unknown),
            ]),
        )
    }

    /// `ProfileHover.from_dict`: missing keys become empty strings, other
    /// values pass through as Python would keep them.
    /// `ProfileHover.from_dict`: `None` for a payload that is not an object
    /// (Python's `data.get` raises there, so nothing is stored or relayed).
    pub fn from_value(data: &Value) -> Option<Hover> {
        let map = data.as_object()?;
        let get = |key: &str| map.get(key).cloned().unwrap_or_else(|| json!(""));
        Some(Hover {
            id: get("id"),
            kind: get("type"),
            from: get("from"),
        })
    }
}

/// Why a profile was refused.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    /// A schema violation: the HTTP layer answers
    /// `"JSON validation error: {message}"`.
    #[error("{0}")]
    Validation(String),
    /// A variable problem (`UndefinedVariableException`, …): the class name
    /// and message, for `"variable error: {class}:{message}"`.
    #[error("{message}")]
    Variable { class: String, message: String },
    #[error("{0}")]
    Other(String),
}

impl From<profile::ProfileError> for ProfileError {
    fn from(error: profile::ProfileError) -> ProfileError {
        match error {
            profile::ProfileError::Schema(err) => ProfileError::Validation(err.message.clone()),
            profile::ProfileError::Preprocess(err) if err.is_variable_error() => {
                ProfileError::Variable {
                    class: err.class_name().to_string(),
                    message: err.to_string(),
                }
            }
            other => ProfileError::Other(other.to_string()),
        }
    }
}

#[derive(Default)]
struct State {
    known: HashMap<String, Value>,
    changes: Vec<Value>,
    default_images: Vec<String>,
    accent_colors: Map<String, Value>,
    known_images: Vec<String>,
    hover: Hover,
    skipped_files: bool,
}

pub struct ProfileManager {
    profiles_dir: PathBuf,
    images_dir: PathBuf,
    default_images_dir: PathBuf,
    config: Arc<ConfigStore>,
    sio: Arc<Sio>,
    state: Mutex<State>,
}

impl ProfileManager {
    pub fn new(
        profiles_dir: PathBuf,
        images_dir: PathBuf,
        default_images_dir: PathBuf,
        config: Arc<ConfigStore>,
        sio: Arc<Sio>,
    ) -> ProfileManager {
        ProfileManager {
            profiles_dir,
            images_dir,
            default_images_dir,
            config,
            sio,
            state: Mutex::new(State::default()),
        }
    }

    /// `ProfileManager.init`.
    pub fn init(&self, app: &App) {
        if let Err(error) = std::fs::create_dir_all(&self.profiles_dir) {
            tracing::error!("Could not create {}: {error}", self.profiles_dir.display());
        }
        self.refresh_image_list();
        app.simple_profile.load();
        self.refresh_profile_list(app);
        self.ensure_manual_mode_profile(app);
        self.delete_unused_images();
        if let Some(Value::Object(last)) = self.last_profile()
            && let Some(profile) = last.get("profile")
        {
            let id = profile
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            self.state.lock().hover = Hover::new(&id, "focus", "backend");
        }
    }

    fn register_change(
        &self,
        change: ProfileEvent,
        profile_id: &str,
        timestamp: Option<f64>,
        change_id: Option<String>,
    ) -> String {
        let change_id = change_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let entry = json!({
            "type": change.value(),
            "profile_id": profile_id,
            "change_id": change_id,
            "timestamp": timestamp.unwrap_or_else(now_seconds),
        });
        let mut state = self.state.lock();
        state.changes.push(entry);
        if state.changes.len() > CHANGES_TO_KEEP {
            let excess = state.changes.len() - CHANGES_TO_KEEP;
            state.changes.drain(..excess);
        }
        change_id
    }

    /// `_emit_profile_event`.
    pub fn emit_event(
        &self,
        change: ProfileEvent,
        profile_id: Option<&str>,
        change_id: Option<&str>,
        brew_type: Option<&str>,
    ) {
        self.sio.emit(
            "profile",
            &profile_event_payload(change, profile_id, change_id, brew_type),
        );
    }

    fn set_last_profile(&self, profile: &Value) {
        self.config.set_and_save(
            keys::PROFILES,
            keys::PROFILE_LAST,
            json!({"load_time": now_seconds(), "profile": profile}),
        );
    }

    /// `ProfileManager._set_last_profile` for callers outside (legacy loads).
    pub fn remember_last_profile(&self, profile: &Value) {
        self.set_last_profile(profile);
    }

    pub fn changes(&self) -> Vec<Value> {
        self.state.lock().changes.clone()
    }

    /// `handle_image`: a random default image when there is none, a data URI
    /// saved as `<md5>.<ext>`, a bare file name turned into the image route.
    pub fn handle_image(&self, data: &mut Value) -> Result<(), ProfileError> {
        // PARITY: `data["display"]` raises KeyError('display') for a profile
        // without one (only save_profile adds it), so loading a schema-valid
        // profile without display answers "failed to load profile 'display'".
        let Some(display) = data
            .as_object_mut()
            .ok_or_else(|| ProfileError::Other("profile is not an object".into()))?
            .get_mut("display")
        else {
            return Err(ProfileError::Other("'display'".into()));
        };
        let Value::Object(display) = display else {
            return Err(ProfileError::Other("'display' is not an object".into()));
        };
        let image = display.get("image").cloned();
        match image {
            None => self.assign_default_image(display),
            Some(Value::String(s)) if s.is_empty() => self.assign_default_image(display),
            Some(Value::String(url)) if !is_relative_url(&url) => match parse_data_uri(&url) {
                Some((media_type, bytes)) => {
                    tracing::info!("The string is a data URI with base64 payload.");
                    if !media_type.starts_with("image/") {
                        tracing::warn!("The data URI does not encode an image.");
                        return Err(ProfileError::Other("Invalid image MIME type".into()));
                    }
                    if bytes.len() > MAX_IMAGE_BYTES {
                        tracing::warn!("File size exceeds limit.");
                        return Err(ProfileError::Other("Image file too large".into()));
                    }
                    let extension = media_type.rsplit('/').next().unwrap_or("png");
                    let filename = format!("{}.{extension}", hex::encode(Md5::digest(&bytes)));
                    std::fs::write(self.images_dir.join(&filename), &bytes)
                        .map_err(|e| ProfileError::Other(format!("Saving file failed: {e}")))?;
                    tracing::info!("File saved as {filename}");
                    display.insert("image".into(), json!(format!("{IMAGE_ROUTE}{filename}")));
                }
                None => tracing::warn!(
                    "The string is neither a relative URL nor a valid data URI with base64 payload."
                ),
            },
            Some(Value::String(url)) if !url.starts_with("/api/v1/profile/image") => {
                display.insert("image".into(), json!(format!("{IMAGE_ROUTE}{url}")));
            }
            _ => {}
        }
        Ok(())
    }

    fn assign_default_image(&self, display: &mut Map<String, Value>) {
        let state = self.state.lock();
        if state.default_images.is_empty() {
            // PARITY: random.choice([]) raises IndexError in Python.
            return;
        }
        let pick = &state.default_images[rand::random_range(0..state.default_images.len())];
        display.insert("image".into(), json!(format!("{IMAGE_ROUTE}{pick}")));
        if let Some(color) = state.accent_colors.get(pick) {
            tracing::info!("using default accent color");
            display.insert("accentColor".into(), color.clone());
        }
    }

    /// `handle_accent_color`.
    fn handle_accent_color(&self, data: &mut Value) {
        let Some(Value::Object(display)) = data.get_mut("display") else {
            return;
        };
        let missing = match display.get("accentColor") {
            None => true,
            Some(Value::String(s)) => s.is_empty(),
            _ => false,
        };
        if !missing {
            return;
        }
        if let Some(Value::String(image)) = display.get("image") {
            let path = image.split(['?', '#']).next().unwrap_or("");
            let base = path.rsplit('/').next().unwrap_or("");
            if let Some(color) = self.state.lock().accent_colors.get(base).cloned() {
                tracing::info!("No accent color found, using default one");
                display.insert("accentColor".into(), color);
                return;
            }
        }
        tracing::info!("No accent color found, generating random one");
        display.insert("accentColor".into(), json!(random_accent_color()));
    }

    /// `save_profile`: assign an id, store images, validate, write the file
    /// and announce the change.
    pub fn save_profile(
        &self,
        app: &App,
        mut data: Value,
        set_last_changed: bool,
        change_id: Option<String>,
        skip_validation: bool,
    ) -> Result<Value, ProfileError> {
        let map = data
            .as_object_mut()
            .ok_or_else(|| ProfileError::Other("'list' object has no attribute 'get'".into()))?;
        if map
            .get("id")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
        {
            map.insert("id".into(), json!(uuid::Uuid::new_v4().to_string()));
        }
        map.entry("display")
            .or_insert_with(|| Value::Object(Map::new()));
        self.handle_image(&mut data)?;
        self.handle_accent_color(&mut data);
        if !skip_validation {
            validate_profile(app, &data)?;
        }
        let now = now_seconds();
        if set_last_changed && let Some(map) = data.as_object_mut() {
            map.insert("last_changed".into(), json!(now));
        }
        let id = data
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let is_update = self.state.lock().known.contains_key(&id);
        let path = self.profiles_dir.join(format!("{id}.json"));
        let text = pyfmt::dumps(&data, &pyfmt::DumpOpts::indented(4));
        std::fs::write(&path, text).map_err(|e| ProfileError::Other(e.to_string()))?;
        self.state.lock().known.insert(id.clone(), data.clone());
        self.add_to_order(&id);
        tracing::info!("Saved profile {id}.json");
        let change = if is_update {
            ProfileEvent::Update
        } else {
            ProfileEvent::Create
        };
        let change_id = self.register_change(change, &id, Some(now), change_id);
        self.emit_event(change, Some(&id), Some(&change_id), None);
        if change == ProfileEvent::Create {
            // The Dial selects a new profile: tell every client.
            self.set_hover(Hover::new(&id, "focus", "dial"));
            self.sio.emit("profileHover", &self.hover().to_value());
        }
        Ok(json!({"profile": data, "change_id": change_id}))
    }

    fn add_to_order(&self, id: &str) {
        let mut order = self.config.get(keys::USER, keys::PROFILE_ORDER);
        let present = order
            .as_array()
            .is_some_and(|ids| ids.iter().any(|v| v.as_str() == Some(id)));
        if !present {
            if let Value::Array(ids) = &mut order {
                ids.push(json!(id));
            } else {
                order = json!([id]);
            }
            self.config.set(keys::USER, keys::PROFILE_ORDER, order);
            self.config.save();
        }
    }

    /// `delete_profile`.
    pub fn delete_profile(
        &self,
        id: &str,
        change_id: Option<String>,
    ) -> Result<Option<Value>, ProfileError> {
        let Some(profile) = self.state.lock().known.get(id).cloned() else {
            return Ok(None);
        };
        let pid = profile
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_string();
        std::fs::remove_file(self.profiles_dir.join(format!("{pid}.json")))
            .map_err(|e| ProfileError::Other(e.to_string()))?;
        self.state.lock().known.remove(&pid);
        let mut order = self.config.get(keys::USER, keys::PROFILE_ORDER);
        if let Value::Array(ids) = &mut order
            && let Some(index) = ids.iter().position(|v| v.as_str() == Some(pid.as_str()))
        {
            ids.remove(index);
            self.config.set(keys::USER, keys::PROFILE_ORDER, order);
            self.config.save();
        }
        // PARITY: Python passes change_id as the timestamp argument here, so the
        // entry's change_id is fresh and its timestamp is the client's id.
        let timestamp_from_client = change_id.clone();
        let change_id = self.register_change_with_raw_timestamp(
            ProfileEvent::Delete,
            &pid,
            timestamp_from_client,
        );
        self.emit_event(ProfileEvent::Delete, Some(&pid), None, None);
        self.delete_unused_images();
        // A deleted profile can no longer be in focus; clear it silently.
        {
            let mut state = self.state.lock();
            if state.hover.id == json!(id) {
                state.hover = Hover::default();
            }
        }
        Ok(Some(json!({"profile": profile, "change_id": change_id})))
    }

    fn register_change_with_raw_timestamp(
        &self,
        change: ProfileEvent,
        profile_id: &str,
        timestamp: Option<String>,
    ) -> String {
        let change_id = uuid::Uuid::new_v4().to_string();
        let timestamp = match timestamp {
            Some(t) => Value::String(t),
            None => json!(now_seconds()),
        };
        let entry = json!({"type": change.value(), "profile_id": profile_id, "change_id": change_id, "timestamp": timestamp});
        let mut state = self.state.lock();
        state.changes.push(entry);
        if state.changes.len() > CHANGES_TO_KEEP {
            let excess = state.changes.len() - CHANGES_TO_KEEP;
            state.changes.drain(..excess);
        }
        change_id
    }

    /// `get_profile`.
    pub fn get_profile(&self, id: &str) -> Option<Value> {
        let profile = self.state.lock().known.get(id).cloned();
        let md5 = profile.as_ref().map(payload_md5);
        tracing::info!(
            "Serving profile: {id} - MD5: {}",
            md5.as_deref().unwrap_or("None")
        );
        profile
    }

    /// `send_profile_to_esp32`: `Ok(None)` when an alarm blocks brewing (the
    /// user is told why), otherwise the profile as stored as the last one.
    pub fn send_profile_to_esp32(
        &self,
        app: &App,
        mut data: Value,
    ) -> Result<Option<Value>, ProfileError> {
        if let Some(end) = app.alarms.end_time(AlarmType::MotorStressed) {
            app.alarms.notify(
                &format!(
                    "Brewing has been disabled because of a recent high strain on the motor, let it rest for {} more minutes",
                    minutes_left(end)
                ),
                WARNING_TRIANGLE_IMAGE,
            );
            return Ok(None);
        }
        if let Some(map) = data.as_object_mut() {
            map.entry("id")
                .or_insert_with(|| json!(uuid::Uuid::new_v4().to_string()));
        }
        self.handle_image(&mut data)?;
        tracing::info!(
            "Recieved <class 'dict'> data with MD5: {}",
            payload_md5(&data)
        );
        tracing::info!("processing simplified profile");
        let started = std::time::Instant::now();
        let program = prepare_for_esp32(app, &data)?;
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        if ms > 10.0 {
            tracing::info!("Preprocessing and variable expansion took {} ms", ms as u64);
        } else {
            tracing::info!(
                "Preprocessing and variable expansion took {} ns",
                (ms * 1000.0) as u64
            );
        }
        tracing::info!(
            "simplified profile streamed to ESP32: data MD5={}",
            payload_md5(&program)
        );
        app.machine.send_json_with_hash(app, &program);
        self.set_last_profile(&data);
        let id = data
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        self.emit_event(ProfileEvent::Load, Some(&id), None, None);
        self.set_hover(Hover::new(&id, "focus", "dial"));
        self.sio.emit("profileHover", &self.hover().to_value());
        Ok(Some(data))
    }

    /// `load_profile_and_send`.
    pub fn load_profile_and_send(
        &self,
        app: &App,
        id: &str,
    ) -> Result<Option<Value>, ProfileError> {
        let profile = self.state.lock().known.get(id).cloned();
        match profile {
            Some(profile) => {
                self.send_profile_to_esp32(app, profile.clone())?;
                Ok(Some(profile))
            }
            None => Ok(None),
        }
    }

    /// `refresh_profile_list`: read every profile file, fill in what older
    /// versions left out, drop what fails validation.
    pub fn refresh_profile_list(&self, app: &App) {
        let started = std::time::Instant::now();
        {
            let mut state = self.state.lock();
            state.known.clear();
            state.skipped_files = false;
        }
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&self.profiles_dir)
            .map(|d| d.filter_map(Result::ok).map(|e| e.path()).collect())
            .unwrap_or_default();
        entries.sort();
        for path in entries {
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(error) => {
                    tracing::warn!("Could not read profile {}: {error}", path.display());
                    self.state.lock().skipped_files = true;
                    continue;
                }
            };
            let mut profile = match pyfmt::loads_lenient(&text) {
                Ok(p) => p,
                Err(error) => {
                    tracing::warn!("Could not decode profile {}: {error}", path.display());
                    self.state.lock().skipped_files = true;
                    continue;
                }
            };
            let changed = normalize_on_load(&mut profile);
            let id = profile
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            tracing::info!("Profile {id} was updated on load");
            if let Err(error) = validate_schema_only(&profile) {
                tracing::warn!(
                    "Profile on disk failed to be loaded {}: {error}",
                    path.display()
                );
                self.state.lock().skipped_files = true;
                continue;
            }
            if changed {
                // `save_profile(profile, set_last_changed=True,
                // skip_validation=True)`: the image and accent color an older
                // profile lacks, the stamp, and the change and its event.
                match self.save_profile(app, profile, true, None, true) {
                    Ok(mut saved) => profile = saved["profile"].take(),
                    Err(error) => {
                        tracing::warn!("Could not save profile {}: {error}", path.display());
                        self.state.lock().skipped_files = true;
                        continue;
                    }
                }
            }
            let newer_known = {
                let state = self.state.lock();
                state.known.get(&id).is_some_and(|known| {
                    known
                        .get("last_changed")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0)
                        >= profile
                            .get("last_changed")
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0)
                })
            };
            if newer_known {
                continue;
            }
            self.state.lock().known.insert(id.clone(), profile);
            self.add_to_order(&id);
        }
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        let count = self.state.lock().known.len();
        let took = if ms > 10.0 {
            format!("{} ms", ms as u64)
        } else {
            format!("{} ns", (ms * 1000.0) as u64)
        };
        tracing::info!("Refreshed profile list in {took} with {count} known profiles.");
        self.emit_event(ProfileEvent::Reload, None, None, None);
    }

    pub fn on_profile_order_changed(&self) {
        tracing::info!("Profile order changed");
        self.emit_event(ProfileEvent::Reload, None, None, None);
    }

    /// `refresh_image_list`: the bundled images copied under their md5 names.
    ///
    /// The images are not built into the binary: they would add 4.6 MB of
    /// PNG to it (on top of the files, which stay the override), and a
    /// package without them still boots, with new profiles left without a
    /// default image and the ERROR below reported. Their accent colors are
    /// built in: without the file a profile showing a bundled image still
    /// gets that image's color, as with the full package.
    pub fn refresh_image_list(&self) {
        tracing::info!("Refreshing default image list");
        if !self.default_images_dir.exists() {
            let _ = std::fs::create_dir_all(&self.default_images_dir);
            tracing::error!("Missing default images path!");
        }
        let _ = std::fs::create_dir_all(&self.images_dir);
        let accent_path = self.default_images_dir.join("accent_colors.json");
        let accent_text = match std::fs::read_to_string(&accent_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tracing::warn!(
                    "No default accent colors at {}, using the built-in ones",
                    accent_path.display()
                );
                Ok(crate::bundled::DEFAULT_ACCENT_COLORS.to_string())
            }
            other => other,
        };
        let accent_colors = match accent_text.map(|t| serde_json::from_str::<Value>(&t)) {
            Ok(Ok(Value::Object(map))) => Some(map),
            Ok(Err(error)) => {
                tracing::warn!(
                    "Could not decode default accent colors {}: {error}",
                    accent_path.display()
                );
                None
            }
            _ => None,
        };
        let mut defaults = Vec::new();
        let mut files: Vec<PathBuf> = std::fs::read_dir(&self.default_images_dir)
            .map(|d| d.filter_map(Result::ok).map(|e| e.path()).collect())
            .unwrap_or_default();
        files.sort();
        for path in files {
            if !path.is_file() {
                continue;
            }
            let extension = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| format!(".{}", e.to_lowercase()))
                .unwrap_or_default();
            if !IMAGE_EXTENSIONS.contains(&extension.as_str()) {
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let name = format!("{}{extension}", hex::encode(Md5::digest(&bytes)));
            if let Err(error) = std::fs::write(self.images_dir.join(&name), &bytes) {
                tracing::warn!("Could not copy {}: {error}", path.display());
            }
            defaults.push(name);
        }
        tracing::info!("Found {} default images", defaults.len());
        let known_images = list_names(&self.images_dir);
        let mut state = self.state.lock();
        if let Some(colors) = accent_colors {
            state.accent_colors = colors;
        }
        state.default_images = defaults;
        state.known_images = known_images;
    }

    /// `_delete_unused_images`.
    pub fn delete_unused_images(&self) {
        if self.state.lock().skipped_files {
            tracing::warn!("Skipped unreadable profiles, keeping all images");
            return;
        }
        tracing::info!("Garbage collecting unused images");
        let (referenced, known, defaults) = {
            let state = self.state.lock();
            let referenced: HashSet<String> = state
                .known
                .values()
                .filter_map(|p| p.get("display")?.get("image")?.as_str().map(str::to_string))
                .filter(|url| url.starts_with(IMAGE_ROUTE))
                .filter_map(|url| url.rsplit('/').next().map(str::to_string))
                .collect();
            (
                referenced,
                state.known_images.clone(),
                state.default_images.clone(),
            )
        };
        for name in known {
            if defaults.contains(&name) || referenced.contains(&name) {
                continue;
            }
            match std::fs::remove_file(self.images_dir.join(&name)) {
                Ok(()) => tracing::info!("Deleted unreferenced image: {name}"),
                Err(error) => tracing::error!("Error deleting file {name}: {error}"),
            }
        }
        self.refresh_image_list();
    }

    pub fn default_images(&self) -> Vec<String> {
        self.state.lock().default_images.clone()
    }

    /// `list_profiles`: in the user's order, the rest after.
    pub fn list_profiles(&self) -> Vec<Value> {
        let mut all = self.state.lock().known.clone();
        let order = self.config.get(keys::USER, keys::PROFILE_ORDER);
        let mut list = Vec::new();
        for id in order
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if let Some(profile) = all.remove(id) {
                list.push(profile);
            }
        }
        let mut rest: Vec<(String, Value)> = all.into_iter().collect();
        rest.sort_by(|a, b| a.0.cmp(&b.0));
        for (id, profile) in rest {
            list.push(profile);
            // Deliberate fix: Python appended a stale loop variable here.
            self.add_to_order(&id);
        }
        list
    }

    /// `get_last_profile`: `{"load_time", "profile"}` or `None`.
    pub fn last_profile(&self) -> Option<Value> {
        match self.config.get(keys::PROFILES, keys::PROFILE_LAST) {
            Value::Null => None,
            other => Some(other),
        }
    }

    pub fn hover(&self) -> Hover {
        self.state.lock().hover.clone()
    }

    pub fn set_hover(&self, hover: Hover) {
        self.state.lock().hover = hover;
    }

    /// `handle_profile_hover`: remember it, tell everyone but the sender.
    pub fn handle_hover(&self, data: &Value, sid: &str) {
        let Some(hover) = Hover::from_value(data) else {
            tracing::error!(
                "profileHover: AttributeError: '{}' object has no attribute 'get'",
                crate::py::type_name(data)
            );
            return;
        };
        tracing::info!(
            "Profile hover updated: {}",
            pyfmt::dumps_default(&hover.to_value())
        );
        self.set_hover(hover.clone());
        self.sio.emit_skip(sid, "profileHover", &hover.to_value());
    }

    /// The hover a new client gets, marked as coming from the backend.
    pub fn emit_hover_to(&self, sid: &str) {
        let mut value = self.hover().to_value();
        if let Value::Object(map) = &mut value {
            map.insert("from".into(), json!("backend"));
        }
        self.sio.emit_to(sid, "profileHover", &value);
    }

    /// `ensure_manual_mode_profile`: seed the Manual mode profile, or give a
    /// damaged or v1 one the seeded stage pair. True when written.
    pub fn ensure_manual_mode_profile(&self, app: &App) -> bool {
        let existing = self
            .state
            .lock()
            .known
            .get(profile::profiles::MANUAL_MODE_PROFILE_ID)
            .cloned();
        let (profile, action) = match profile::plan_manual_mode_profile(existing.as_ref()) {
            profile::ManualModeSeed::UpToDate => return false,
            profile::ManualModeSeed::Seed(p) => (p, "Seeded"),
            profile::ManualModeSeed::Migrate(p) => (p, "Migrated"),
        };
        match self.save_profile(app, profile, true, None, false) {
            Ok(_) => {
                tracing::info!("{action} Manual mode profile");
                true
            }
            Err(error) => {
                // Best effort: a machine that cannot write it still starts.
                tracing::error!("Failed to write the Manual mode profile: {error}");
                false
            }
        }
    }

    pub fn known_count(&self) -> usize {
        self.state.lock().known.len()
    }
}

/// `LEGACY_DUMMY_PROFILE` (`api/emulation.py`): what a legacy JSON load
/// records as the last profile.
pub fn legacy_dummy_profile() -> Value {
    json!({
        "id": "0eddc1d9-aa2e-404b-a209-e7e76c9552bc",
        "name": "LEGACY JSON PLACEHOLDER",
        "author": "",
        "author_id": "00000000-0000-0000-0000-000000000000",
        "previous_authors": [{"name": "", "author_id": "", "profile_id": ""}],
        "temperature": 88,
        "final_weight": 36,
        "display": {
            "image": "/api/v1/profile/image/f98ff6e029e70ee9a99565796615d55c.png",
            "accentColor": "#42a59d",
        },
        "variables": [],
        "last_changed": 1723127046.5605135,
        "stages": [{
            "key": "206c654b-5766-4ebb-a4bc-dcafa5aacd8b",
            "name": "EMPTY FLOW STAGE",
            "type": "flow",
            "dynamics": {"points": [[0, 0]], "over": "time", "interpolation": "linear"},
            "exit_triggers": [],
            "limits": [],
        }],
    })
}

/// `_get_payload_md5`: md5 of `json.dumps(payload, sort_keys=True)`.
pub fn payload_md5(payload: &Value) -> String {
    let opts = pyfmt::DumpOpts {
        sort_keys: true,
        ..pyfmt::DumpOpts::default()
    };
    hex::encode(Md5::digest(pyfmt::dumps(payload, &opts).as_bytes()))
}

fn is_relative_url(url: &str) -> bool {
    // urlparse: no scheme and no netloc.
    let has_scheme = url.split_once(':').is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
            && scheme
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
    });
    !has_scheme && !url.starts_with("//")
}

/// `datauri.parse`: `data:<media type>[;base64],<payload>`.
fn parse_data_uri(uri: &str) -> Option<(String, Vec<u8>)> {
    use base64::Engine;
    let rest = uri.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let mut parts = meta.split(';');
    let media_type = parts
        .next()
        .filter(|m| !m.is_empty())
        .unwrap_or("text/plain")
        .to_string();
    let base64 = meta.split(';').any(|p| p == "base64");
    let bytes = if base64 {
        base64::engine::general_purpose::STANDARD
            .decode(payload.trim())
            .ok()?
    } else {
        pyfmt::unquote(payload).into_bytes()
    };
    Some((media_type, bytes))
}

fn random_accent_color() -> String {
    format!("#{:06x}", rand::random_range(0u32..(1 << 24)))
}

fn list_names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .map(|d| {
            d.filter_map(Result::ok)
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// What `refresh_profile_list` fills in on profiles older versions wrote.
fn normalize_on_load(profile: &mut Value) -> bool {
    let mut new_uuid = || uuid::Uuid::new_v4().to_string();
    // Deliberate fix: a profile file that is not an object makes Python's
    // refresh raise and the backend fail to start; it is skipped instead.
    profile::normalize_loaded_profile(profile, &mut new_uuid).unwrap_or(false)
}

fn validate_profile(app: &App, data: &Value) -> Result<(), ProfileError> {
    let _ = app;
    validate_schema_only(data)
}

/// `validate_profile`: the variables resolve and the schema holds.
fn validate_schema_only(data: &Value) -> Result<(), ProfileError> {
    if let Some(id) = data.get("id") {
        let id = match id {
            Value::String(s) => s.clone(),
            other => crate::metconfig::py_repr(other),
        };
        tracing::info!("validating profile: {id}");
    }
    profile::validate_profile(data).map_err(|error| {
        if let profile::ProfileError::Schema(err) = &error {
            tracing::error!("JSON validation error: {}", err.message);
        } else {
            tracing::info!(
                "Profile variables could not be processed: {}: {error}",
                error.class_name()
            );
        }
        ProfileError::from(error)
    })?;
    tracing::info!("JSON data is valid.");
    Ok(())
}

/// What `send_profile_to_esp32` streams: validated, variables expanded,
/// a manual profile turned into its node program, cut to the ESP32 fields.
fn prepare_for_esp32(app: &App, data: &Value) -> Result<Value, ProfileError> {
    let settings = profile::ProgramSettings {
        auto_purge: app.config.get_bool(keys::USER, keys::PROFILE_AUTO_PURGE),
        allow_stage_skipping: app
            .config
            .get_bool(keys::USER, keys::MACHINE_ALLOW_STAGE_SKIPPING),
        partial_retraction: app
            .config
            .get_f64(keys::USER, keys::PROFILE_PARTIAL_RETRACTION)
            .unwrap_or(crate::config::PARTIAL_RETRACTION_DEFAULT),
    };
    let mut ids = app.reference_ids.lock();
    Ok(profile::prepare_for_esp32(data, &settings, &mut ids)?)
}

#[cfg(test)]
mod hover_tests {
    use serde_json::json;

    /// : the values are relayed as the client sent
    /// them, a missing key is "", and a payload that is not an object is
    /// neither stored nor relayed (Python raises in the handler).
    #[test]
    fn hovers_are_relayed_as_sent() {
        let (_dir, app) = crate::testing::app();
        app.sio.clear_emitted();
        app.profiles
            .handle_hover(&json!({"id": 5, "type": null, "extra": 1}), "sid-1");
        let emitted = app.sio.emitted();
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].event, "profileHover");
        assert_eq!(emitted[0].data, json!({"id": 5, "type": null, "from": ""}));
        assert_eq!(emitted[0].skip_sid.as_deref(), Some("sid-1"));

        app.sio.clear_emitted();
        app.profiles.handle_hover(&json!("p-1"), "sid-1");
        assert!(app.sio.emitted().is_empty());
        assert_eq!(app.profiles.hover().id, json!(5));
    }
}
