//! The services the Python keeps in module-level singletons, owned in one
//! place and handed to whoever needs them.
//!
//! Services that call each other (the machine loads a profile, the profile
//! manager streams it to the machine) take `&App` in those methods instead of
//! holding references to each other, so there are no reference cycles and no
//! globals.

use std::sync::Arc;

use crate::alarms::AlarmManager;
use crate::backlight::BacklightController;
use crate::config::{self, ConfigStore};
use crate::dbus_monitor::DbusMonitor;
use crate::env::Env;
use crate::host::Host;
use crate::imager::DiscImager;
use crate::limited_access::UnlockThrottle;
use crate::logging::LogHooks;
use crate::notifications::NotificationManager;
use crate::os_update::OsUpdateStatus;
use crate::ota::UpdateManager;
use crate::sio::Sio;
use crate::smoke_validation::SmokeValidationManager;
use crate::sounds::SoundPlayer;
use crate::ssh_manager::SshManager;
use crate::system_services::SystemServices;
use crate::usb::UsbManager;

pub struct App {
    pub env: Env,
    pub log: Arc<LogHooks>,
    pub config: Arc<ConfigStore>,
    pub sio: Arc<Sio>,
    pub sounds: Arc<SoundPlayer>,
    pub notifications: Arc<NotificationManager>,
    pub alarms: Arc<AlarmManager>,
    /// The commands of the machine image (`subprocess`).
    pub host: Arc<Host>,
    /// `UpdateOSStatus` (`api/machine.py`).
    pub os_update: Arc<OsUpdateStatus>,
    /// `UpdateManager` (`ota.py`).
    pub ota: Arc<UpdateManager>,
    /// `DBusMonitor`.
    pub dbus: Arc<DbusMonitor>,
    pub system_services: Arc<SystemServices>,
    /// `SSHManager`.
    pub ssh: Arc<SshManager>,
    /// `USBManager`.
    pub usb: Arc<UsbManager>,
    pub backlight: Arc<BacklightController>,
    /// `DiscImager`.
    pub imager: Arc<DiscImager>,
    /// `limited_access.UnlockThrottle`.
    pub unlock_throttle: Arc<UnlockThrottle>,
    /// `SmokeValidationManager`.
    pub smoke: Arc<SmokeValidationManager>,
}

impl App {
    /// Build the services in the order the Python imports and initializes
    /// them. Nothing here touches hardware or starts tasks; `start` does.
    pub fn new(env: Env, log: Arc<LogHooks>) -> Arc<App> {
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
        let host = Arc::new(Host::default());
        let system = &env.paths.system;
        let os_update = Arc::new(OsUpdateStatus::new(sio.clone()));
        let ota = Arc::new(UpdateManager::new(config.clone(), system));
        let dbus = Arc::new(DbusMonitor::new(
            config.clone(),
            notifications.clone(),
            os_update.clone(),
            host.clone(),
            system.usb_test_mount.clone(),
        ));
        let system_services =
            Arc::new(SystemServices::new(config.clone(), env.hooks.fake_hardware));
        let ssh = Arc::new(SshManager::new(
            config.clone(),
            host.clone(),
            system_services.clone(),
            system.ssh_issue.clone(),
        ));
        let usb = Arc::new(UsbManager::new(
            config.clone(),
            host.clone(),
            env.hooks.fake_hardware,
        ));
        let backlight = Arc::new(BacklightController::new(system.backlight.clone()));
        let imager = Arc::new(DiscImager::new(
            notifications.clone(),
            host.clone(),
            system.provisioning.clone(),
            env.hooks.fake_hardware,
        ));
        let smoke = Arc::new(SmokeValidationManager::new(
            env.paths.smoke_validation_state.clone(),
            env.paths.smoke_validation_version.clone(),
            system.smoke_publish_command.clone(),
            host.clone(),
        ));
        Arc::new(App {
            env,
            log,
            config,
            sio,
            sounds,
            notifications,
            alarms,
            host,
            os_update,
            ota,
            dbus,
            system_services,
            ssh,
            usb,
            backlight,
            imager,
            unlock_throttle: Arc::new(UnlockThrottle::default()),
            smoke,
        })
    }

    pub fn emulated(&self) -> bool {
        self.env.emulated()
    }
}
