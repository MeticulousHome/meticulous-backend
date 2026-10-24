//! The services the Python keeps in module-level singletons, owned in one
//! place and handed to whoever needs them.
//!
//! Services that call each other (the machine loads a profile, the profile
//! manager streams it to the machine) take `&App` in those methods instead of
//! holding references to each other, so there are no reference cycles and no
//! globals.

use std::sync::Arc;

use crate::alarms::AlarmManager;
use crate::config::{self, ConfigStore};
use crate::debug_shots::ShotDebugManager;
use crate::env::Env;
use crate::logging::LogHooks;
use crate::machine::Machine;
use crate::notifications::NotificationManager;
use crate::pending;
use crate::pour_over::PourOverProfileManager;
use crate::profiles::ProfileManager;
use crate::redaction::Redaction;
use crate::sentry_reporting::Reporting;
use crate::shots::ShotManager;
use crate::simple_profile::SimpleProfile;
use crate::sio::Sio;
use crate::sounds::SoundPlayer;

pub struct App {
    pub env: Env,
    pub log: Arc<LogHooks>,
    pub reporting: Arc<Reporting>,
    pub redaction: Arc<Redaction>,
    pub config: Arc<ConfigStore>,
    pub sio: Arc<Sio>,
    pub sounds: Arc<SoundPlayer>,
    pub notifications: Arc<NotificationManager>,
    pub alarms: Arc<AlarmManager>,
    pub machine: Machine,
    pub shots: ShotManager,
    pub debug_shots: ShotDebugManager,
    pub profiles: ProfileManager,
    pub simple_profile: SimpleProfile,
    pub pour_over: PourOverProfileManager,
    pub history: pending::History,
    pub ssh: pending::Ssh,
    pub smoke: pending::Smoke,
    pub ota: pending::Ota,
    pub usb: pending::Usb,
    pub timezone: pending::Timezone,
    pub shot_sharing: pending::ShotSharing,
    /// The node program's reference ids, which never reset (PARITY: the
    /// Python counter lives for the process).
    pub reference_ids: parking_lot::Mutex<profile::ReferenceIds>,
}

impl App {
    /// Build the services in the order the Python imports and initializes
    /// them. Nothing here touches hardware or starts tasks; `start` does.
    pub fn new(env: Env, log: Arc<LogHooks>, reporting: Arc<Reporting>) -> Arc<App> {
        let redaction = Redaction::new(env.paths.redaction_key.clone());
        let config = Arc::new(ConfigStore::open(
            env.paths.config_file(),
            config::defaults(),
        ));
        let sio = Arc::new(Sio::default());
        let sounds = Arc::new(SoundPlayer::new(
            config.clone(),
            env.paths.system_sounds.clone(),
            env.paths.user_sounds.clone(),
            env.hooks.fake_hardware,
        ));
        let notifications = Arc::new(NotificationManager::new(
            sio.clone(),
            config.clone(),
            sounds.clone(),
        ));
        let alarms = Arc::new(AlarmManager::new(
            env.paths.alarms.clone(),
            notifications.clone(),
        ));
        let machine = Machine::new(env.backend);
        let shots = ShotManager::new(env.paths.shots());
        let debug_shots = ShotDebugManager::new(env.paths.debug_history.clone());
        let profiles = ProfileManager::new(
            env.paths.profiles.clone(),
            env.paths.images.clone(),
            env.paths.default_images.clone(),
            config.clone(),
            sio.clone(),
        );
        let simple_profile = SimpleProfile::new(env.paths.simple_profile.clone());
        let env_paths_pour_over = env.paths.pour_over_profiles.clone();
        Arc::new(App {
            env,
            log,
            reporting,
            redaction,
            config,
            sio,
            sounds,
            notifications,
            alarms,
            machine,
            shots,
            debug_shots,
            profiles,
            simple_profile,
            pour_over: PourOverProfileManager::new(env_paths_pour_over),
            history: pending::History,
            ssh: pending::Ssh,
            smoke: pending::Smoke,
            ota: pending::Ota,
            usb: pending::Usb,
            timezone: pending::Timezone,
            shot_sharing: pending::ShotSharing,
            reference_ids: parking_lot::Mutex::new(profile::ReferenceIds::default()),
        })
    }

    pub fn emulated(&self) -> bool {
        self.env.emulated()
    }
}
