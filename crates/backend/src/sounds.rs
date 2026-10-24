//! `sounds.py`: event sounds from a theme folder, played with gst-play.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{Map, Value};

use crate::config::{ConfigStore, keys};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundEvent {
    Startup,
    HeatingStart,
    HeatingEnd,
    BrewingStart,
    BrewingEnd,
    Abort,
    Idle,
    Notification,
}

impl SoundEvent {
    /// `sound_event.name.lower()`: the key in a theme's config.json.
    pub fn key(self) -> &'static str {
        match self {
            SoundEvent::Startup => "startup",
            SoundEvent::HeatingStart => "heating_start",
            SoundEvent::HeatingEnd => "heating_end",
            SoundEvent::BrewingStart => "brewing_start",
            SoundEvent::BrewingEnd => "brewing_end",
            SoundEvent::Abort => "abort",
            SoundEvent::Idle => "idle",
            SoundEvent::Notification => "notification",
        }
    }
}

pub const DEFAULT_THEME: &str = "default";
const AUDIO_ENABLE_GPIO_CHIP: &str = "/dev/gpiochip4";
const AUDIO_ENABLE_GPIO_LINE: u32 = 25;

#[derive(Default)]
struct State {
    /// `None` until `init` ran (`KNOWN_THEMES is None`).
    known_themes: Option<BTreeMap<String, PathBuf>>,
    /// PARITY: Python's CURRENT_THEME_CONFIG is DEFAULT_THEME_CONFIG, updated
    /// in place by every theme set, so keys of earlier themes linger.
    theme_config: Map<String, Value>,
    current_theme: Option<String>,
    playing: Option<Child>,
}

pub struct SoundPlayer {
    config: Arc<ConfigStore>,
    system_sounds: PathBuf,
    user_sounds: PathBuf,
    fake_hardware: bool,
    state: Mutex<State>,
    audio_pin: Mutex<Option<gpiocdev::Request>>,
}

impl SoundPlayer {
    pub fn new(
        config: Arc<ConfigStore>,
        system_sounds: PathBuf,
        user_sounds: PathBuf,
        fake_hardware: bool,
    ) -> SoundPlayer {
        SoundPlayer {
            config,
            system_sounds,
            user_sounds,
            fake_hardware,
            state: Mutex::new(State::default()),
            audio_pin: Mutex::new(None),
        }
    }

    /// `SoundPlayer.init`: enable the amplifier, find the themes, set the
    /// volume and the configured theme, play the startup sound.
    pub fn init(&self, emulation: bool, play_startup_sound: bool) {
        if !emulation && !self.fake_hardware {
            match gpiocdev::Request::builder()
                .on_chip(AUDIO_ENABLE_GPIO_CHIP)
                .with_consumer("sounds")
                .with_line(AUDIO_ENABLE_GPIO_LINE)
                .as_output(gpiocdev::line::Value::Active)
                .request()
            {
                Ok(request) => *self.audio_pin.lock() = Some(request),
                Err(error) => tracing::error!("Failed to set GPIO pin: {error}"),
            }
        }

        let mut themes = scan_folder(&self.user_sounds);
        themes.extend(scan_folder(&self.system_sounds));
        {
            let mut state = self.state.lock();
            state.theme_config = load_theme(&themes, DEFAULT_THEME).unwrap_or_default();
            state.known_themes = Some(themes);
        }

        if !self.fake_hardware {
            if let Err(error) = Command::new("pactl")
                .args(["--", "set-sink-volume", "0", "40%"])
                .status()
            {
                tracing::error!("failed to set audio volume: {error}");
            }
        }

        let theme = self.config.get(keys::SYSTEM, keys::SOUNDS_THEME);
        self.set_theme(theme.as_str());
        if play_startup_sound {
            self.play_event(SoundEvent::Startup);
        }
    }

    pub fn available_themes(&self) -> Vec<String> {
        self.state
            .lock()
            .known_themes
            .as_ref()
            .map(|t| t.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// `get_theme()`: the merged theme config.
    pub fn theme_config(&self) -> Map<String, Value> {
        self.state.lock().theme_config.clone()
    }

    /// `set_theme(name)`: `None` is accepted like Python's `theme_name is None`.
    pub fn set_theme(&self, name: Option<&str>) -> bool {
        let known = {
            let state = self.state.lock();
            match &state.known_themes {
                None => {
                    tracing::warn!(
                        "Manipulating theme before SoundPlayer was initialized. Ignoring."
                    );
                    return false;
                }
                Some(themes) => name.is_none_or(|n| themes.contains_key(n)),
            }
        };
        if known {
            self.config.set_and_save(
                keys::SYSTEM,
                keys::SOUNDS_THEME,
                name.map_or(Value::Null, |n| Value::String(n.to_string())),
            );
            let mut state = self.state.lock();
            let themes = state.known_themes.clone().unwrap_or_default();
            if let Some(config) = name.and_then(|n| load_theme(&themes, n)) {
                for (k, v) in config {
                    state.theme_config.insert(k, v);
                }
            }
            state.current_theme = name.map(str::to_string);
            tracing::info!("Sound theme '{}' loaded", name.unwrap_or("None"));
            true
        } else {
            tracing::error!(
                "Error: Theme '{}' is not available.",
                name.unwrap_or("None")
            );
            if name != Some(DEFAULT_THEME) {
                self.set_theme(Some(DEFAULT_THEME));
            }
            false
        }
    }

    pub fn play_event(&self, event: SoundEvent) -> bool {
        self.play(event.key())
    }

    /// `play_sound(name)`: true when played or deliberately silent.
    pub fn play(&self, name: &str) -> bool {
        if !self.config.get_bool(keys::USER, keys::SOUNDS_ENABLED) {
            tracing::info!("No sounds enabled");
            return true;
        }
        let configured = self.config.get_str(keys::SYSTEM, keys::SOUNDS_THEME);
        {
            let state = self.state.lock();
            if state.known_themes.is_none() {
                tracing::warn!("Playing sound before SoundPlayer was initialized. Ignoring.");
                return false;
            }
            if state.current_theme != configured {
                drop(state);
                self.set_theme(configured.as_deref());
            }
        }
        let mut state = self.state.lock();
        let Some(entry) = state.theme_config.get(name).cloned() else {
            tracing::info!("Sound not found");
            return false;
        };
        let file_name = match &entry {
            Value::String(s) if !s.is_empty() => s.clone(),
            Value::Null => {
                tracing::info!("Sound {name} is disabled by the theme");
                return true;
            }
            Value::String(_) => {
                tracing::info!("Sound {name} is disabled by the theme");
                return true;
            }
            Value::Object(o) if o.is_empty() => {
                tracing::info!("Sound {name} is disabled by the theme");
                return true;
            }
            other => other.to_string(),
        };
        let theme_path = state
            .current_theme
            .as_ref()
            .and_then(|t| state.known_themes.as_ref().and_then(|k| k.get(t)))
            .cloned()
            .unwrap_or_default();
        let file_path = theme_path.join(&file_name);
        tracing::info!("Playing {name} from {}", file_path.display());
        if !file_path.exists() {
            tracing::warn!("Sound file not found: {}", file_path.display());
            return false;
        }
        if self.fake_hardware {
            return true;
        }
        if let Some(mut previous) = state.playing.take() {
            let _ = previous.kill();
            let _ = previous.wait();
        }
        match Command::new("gst-play-1.0")
            .args(["--no-interactive", "--quiet"])
            .arg(&file_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => {
                state.playing = Some(child);
                true
            }
            Err(error) => {
                tracing::error!("Failed to play sound: {error}");
                false
            }
        }
    }
}

fn scan_folder(folder: &Path) -> BTreeMap<String, PathBuf> {
    let mut themes = BTreeMap::new();
    tracing::info!("Scanning Theme folder {}", folder.display());
    let mut stack = vec![folder.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if has_config(&path) {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        tracing::info!("Found sound theme '{}'", path.display());
                        themes.insert(name.to_string(), path.clone());
                    }
                } else {
                    tracing::info!("'{}' has no config.json", path.display());
                }
                stack.push(path);
            }
        }
    }
    themes
}

fn has_config(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("config.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .is_some()
}

fn load_theme(themes: &BTreeMap<String, PathBuf>, name: &str) -> Option<Map<String, Value>> {
    let folder = themes.get(name)?;
    let text = std::fs::read_to_string(folder.join("config.json")).ok()?;
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Some(map),
        _ => Some(Map::new()),
    }
}
