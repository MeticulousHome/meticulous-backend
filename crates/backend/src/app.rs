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
use crate::env::Env;
use crate::logging::LogHooks;
use crate::notifications::NotificationManager;
use crate::redaction::Redaction;
use crate::sentry_reporting::Reporting;
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
        })
    }

    pub fn emulated(&self) -> bool {
        self.env.emulated()
    }
}
