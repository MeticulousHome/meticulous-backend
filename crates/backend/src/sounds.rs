//! `sounds.py`: event sounds from a theme folder, played with gst-play.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{Map, Value};

use crate::config::{ConfigStore, keys};
use crate::host::{Host, Process};

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
/// playsound3's `gst_play` backend: what `playsound(path, block=False)` runs.
const PLAYER: [&str; 3] = ["gst-play-1.0", "--no-interactive", "--quiet"];

/// `KNOWN_THEMES`: theme name to folder, in the order of the Python dict
/// (user themes first, then system themes; a system theme with a user
/// theme's name takes its folder and keeps its place).
type Themes = Vec<(String, Theme)>;

#[derive(Debug, Clone)]
struct Theme {
    folder: PathBuf,
    /// The system default theme without its `config.json`: the theme's
    /// config is the one built into the binary.
    built_in: bool,
}

impl Theme {
    fn on_disk(folder: PathBuf) -> Theme {
        Theme {
            folder,
            built_in: false,
        }
    }
}

#[derive(Default)]
struct State {
    /// `None` until `init` ran (`KNOWN_THEMES is None`).
    known_themes: Option<Themes>,
    /// PARITY: Python's CURRENT_THEME_CONFIG is DEFAULT_THEME_CONFIG, updated
    /// in place by every theme set, so keys of earlier themes linger.
    theme_config: Map<String, Value>,
    current_theme: Option<String>,
    playing: Option<Box<dyn Process>>,
}

pub struct SoundPlayer {
    config: Arc<ConfigStore>,
    host: Arc<Host>,
    system_sounds: PathBuf,
    user_sounds: PathBuf,
    fake_hardware: bool,
    state: Mutex<State>,
    audio_pin: Mutex<Option<gpiocdev::Request>>,
    /// Every event asked for, played or not: what the Python suite records
    /// in place of `SoundPlayer.play_event_sound`.
    #[cfg(test)]
    pub events: Mutex<Vec<SoundEvent>>,
}

impl SoundPlayer {
    pub fn new(
        config: Arc<ConfigStore>,
        host: Arc<Host>,
        system_sounds: PathBuf,
        user_sounds: PathBuf,
        fake_hardware: bool,
    ) -> SoundPlayer {
        SoundPlayer {
            config,
            host,
            system_sounds,
            user_sounds,
            fake_hardware,
            state: Mutex::new(State::default()),
            audio_pin: Mutex::new(None),
            #[cfg(test)]
            events: Mutex::new(Vec::new()),
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

        let mut themes = Themes::new();
        for (name, theme) in scan_folder(&self.user_sounds)
            .into_iter()
            .chain(scan_folder(&self.system_sounds))
        {
            insert(&mut themes, name, theme);
        }
        // Deliberate change: a package without the default theme's
        // config.json still has the theme (and serves its config), where
        // Python has no theme at all and logs errors on every set and play.
        // Its sounds play from the system folder if they are there.
        if folder(&themes, DEFAULT_THEME).is_none() {
            tracing::warn!(
                "No sound theme '{DEFAULT_THEME}' with a config.json in {}, using the built-in config",
                self.system_sounds.display()
            );
            themes.push((
                DEFAULT_THEME.to_string(),
                Theme {
                    folder: self.system_sounds.join(DEFAULT_THEME),
                    built_in: true,
                },
            ));
        }
        {
            let mut state = self.state.lock();
            state.theme_config = load_theme(&themes, DEFAULT_THEME).unwrap_or_default();
            state.known_themes = Some(themes);
        }

        if !self.fake_hardware
            && let Err(error) = self
                .host
                .run(&["pactl", "--", "set-sink-volume", "0", "40%"])
        {
            tracing::error!("failed to set audio volume: {error}");
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
            .map(|t| t.iter().map(|(name, _)| name.clone()).collect())
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
                Some(themes) => name.is_none_or(|n| folder(themes, n).is_some()),
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
            // Deliberate fix: for a cleared theme (`None`) Python runs
            // `update(_load_theme(None))`, i.e. `update(False)`, a TypeError
            // out of init or play_sound; the merged config stays as it is.
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
        #[cfg(test)]
        self.events.lock().push(event);
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
            .and_then(|t| state.known_themes.as_ref().and_then(|k| folder(k, t)))
            .cloned()
            .unwrap_or_default();
        let file_path = crate::http::absolute(&theme_path.join(&file_name));
        tracing::info!("Playing {name} from {}", file_path.display());
        if !file_path.exists() {
            tracing::warn!("Sound file not found: {}", file_path.display());
            return false;
        }
        if self.fake_hardware {
            return true;
        }
        if let Some(mut previous) = state.playing.take() {
            previous.stop();
        }
        let file = file_path.to_string_lossy();
        let mut argv = PLAYER.to_vec();
        argv.push(&file);
        match self.host.start(&argv) {
            Ok(process) => {
                state.playing = Some(process);
                true
            }
            Err(error) => {
                tracing::error!("Failed to play sound: {error}");
                false
            }
        }
    }
}

fn theme<'a>(themes: &'a Themes, name: &str) -> Option<&'a Theme> {
    themes.iter().find(|(n, _)| n == name).map(|(_, t)| t)
}

fn folder<'a>(themes: &'a Themes, name: &str) -> Option<&'a PathBuf> {
    theme(themes, name).map(|t| &t.folder)
}

/// `dict[name] = folder`: a known name keeps its place.
fn insert(themes: &mut Themes, name: String, theme: Theme) {
    match themes.iter_mut().find(|(n, _)| *n == name) {
        Some(entry) => entry.1 = theme,
        None => themes.push((name, theme)),
    }
}

/// `scan_folder`: every directory below `folder` with a readable config.json,
/// in `os.walk` order (top-down; each directory's subdirectories in listing
/// order before any of theirs; symlinked directories listed, not entered).
fn scan_folder(folder: &Path) -> Themes {
    tracing::info!("Scanning Theme folder {}", folder.display());
    let mut themes = Themes::new();
    walk(folder, &mut themes);
    themes
}

fn walk(dir: &Path, themes: &mut Themes) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let subdirs: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    for path in &subdirs {
        if has_config(path) {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                tracing::info!("Found sound theme '{}'", path.display());
                insert(themes, name.to_string(), Theme::on_disk(path.clone()));
            }
        } else {
            tracing::info!("'{}' has no config.json", path.display());
        }
    }
    for path in subdirs {
        if !path.is_symlink() {
            walk(&path, themes);
        }
    }
}

fn has_config(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("config.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .is_some()
}

fn load_theme(themes: &Themes, name: &str) -> Option<Map<String, Value>> {
    let theme = theme(themes, name)?;
    let text = if theme.built_in {
        crate::bundled::DEFAULT_SOUND_THEME.to_string()
    } else {
        std::fs::read_to_string(theme.folder.join("config.json")).ok()?
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Some(map),
        _ => Some(Map::new()),
    }
}
