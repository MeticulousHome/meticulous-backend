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
use crate::ble::GattServer;
use crate::bug_report::{BugReports, ReportsConfig};
use crate::config::{self, ConfigStore};
use crate::dbus_monitor::DbusMonitor;
use crate::debug_shots::ShotDebugManager;
use crate::env::Env;
use crate::host::Host;
use crate::hostname::HostnameManager;
use crate::http_client::{HttpClient, ReqwestClient};
use crate::imager::DiscImager;
use crate::limited_access::UnlockThrottle;
use crate::logging::LogHooks;
use crate::machine::Machine;
use crate::notifications::NotificationManager;
use crate::os_update::OsUpdateStatus;
use crate::ota::UpdateManager;
use crate::pour_over::PourOverProfileManager;
use crate::profiles::ProfileManager;
use crate::py::{CommandRunner, PyError, SystemRunner};
use crate::redaction::Redaction;
use crate::sentry_reporting::Reporting;
use crate::shot_data_sharing::{SharingTarget, ShotDataSharing};
use crate::shots::ShotManager;
use crate::simple_profile::SimpleProfile;
use crate::sio::Sio;
use crate::smoke_validation::SmokeValidationManager;
use crate::sounds::SoundPlayer;
use crate::ssh_manager::SshManager;
use crate::system_services::SystemServices;
use crate::timezone::TimezoneManager;
use crate::usb::UsbManager;
use crate::wifi::{AnnouncerFactory, SystemWifiHost, WifiManager};
use crate::zeroconf::{Announcer, MdnsResponder, ZeroConfAnnouncement};

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
    pub history: crate::shot_database::History,
    pub shot_sharing: ShotDataSharing,
    pub reports: BugReports,
    /// The node program's reference ids, which never reset (PARITY: the
    /// Python counter lives for the process).
    pub reference_ids: parking_lot::Mutex<profile::ReferenceIds>,
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
    pub hostname: Arc<HostnameManager>,
    pub timezone: Arc<TimezoneManager>,
    pub wifi: Arc<WifiManager>,
    pub gatt: Arc<GattServer>,
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
        // MeticulousConfig.setSIO(sio): every save tells the clients to
        // reload the settings. Saves before the server runs reach nobody.
        {
            let sio = sio.clone();
            config.on_save(move || sio.emit("settings", &serde_json::json!({})));
        }
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
        let history = crate::shot_database::History::new(history::HistoryPaths::new(
            env.paths.history.clone(),
            env.paths.user_db_migrations.clone(),
        ));
        let profiles = ProfileManager::new(
            env.paths.profiles.clone(),
            env.paths.images.clone(),
            env.paths.default_images.clone(),
            config.clone(),
            sio.clone(),
        );
        let simple_profile = SimpleProfile::new(env.paths.simple_profile.clone());
        let env_paths_pour_over = env.paths.pour_over_profiles.clone();
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
        let runner: Arc<dyn CommandRunner> = Arc::new(SystemRunner);
        let http: Arc<dyn HttpClient> = Arc::new(ReqwestClient::new());
        let hostname = Arc::new(HostnameManager::new(config.clone(), runner.clone()));
        let timezone = Arc::new(TimezoneManager::new(
            config.clone(),
            env.paths.timezone_json.clone(),
            runner,
            http.clone(),
        ));
        let wifi = WifiManager::new(
            config.clone(),
            hostname.clone(),
            timezone.clone(),
            Arc::new(SystemWifiHost::new(http)),
            env.paths.redaction_key.clone(),
            mdns_announcer(),
        );
        let gatt = GattServer::new(wifi.clone(), hostname.clone(), config.clone(), env.port);
        // Tests never read the developer's environment.
        let lookup = |name: &str| {
            if cfg!(test) {
                None
            } else {
                std::env::var(name).ok()
            }
        };
        let reports = BugReports::new(ReportsConfig::from_lookup(&env.paths, lookup));
        let shot_sharing = ShotDataSharing::new(SharingTarget::from_lookup(lookup));
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
            history,
            shot_sharing,
            reports,
            reference_ids: parking_lot::Mutex::new(profile::ReferenceIds::default()),
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
            hostname,
            timezone,
            wifi,
            gatt,
        })
    }

    pub fn emulated(&self) -> bool {
        self.env.emulated()
    }
}

/// `ZeroConfAnnouncement(config_function=WifiManager.getCurrentConfig)` on
/// mDNS, built when the Wi-Fi manager initializes.
fn mdns_announcer() -> AnnouncerFactory {
    Arc::new(|wifi| {
        let responder = Arc::new(MdnsResponder::new().map_err(|e| PyError::new("OSError", e))?);
        let source = Box::new(move || match wifi.upgrade() {
            Some(wifi) => wifi.get_current_config(),
            None => Err(PyError::new("RuntimeError", "the Wi-Fi manager is gone")),
        });
        let announcement = Arc::new(ZeroConfAnnouncement::new(
            source,
            responder.clone(),
            ZeroConfAnnouncement::port_from_env(),
        ));
        let weak = Arc::downgrade(&announcement);
        responder.on_addresses_changed(move || {
            if let Some(announcement) = weak.upgrade() {
                announcement.refresh();
            }
        });
        let announcer: Arc<dyn Announcer> = announcement;
        Ok(announcer)
    })
}
