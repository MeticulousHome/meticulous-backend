//! The services the Python keeps in module-level singletons, owned in one
//! place and handed to whoever needs them.
//!
//! Services that call each other (the machine loads a profile, the profile
//! manager streams it to the machine) take `&App` in those methods instead of
//! holding references to each other, so there are no reference cycles and no
//! globals.

use std::sync::Arc;

use crate::alarms::AlarmManager;
use crate::ble::GattServer;
use crate::config::{self, ConfigStore};
use crate::env::Env;
use crate::hostname::HostnameManager;
use crate::http_client::{HttpClient, ReqwestClient};
use crate::logging::LogHooks;
use crate::notifications::NotificationManager;
use crate::py::{CommandRunner, PyError, SystemRunner};
use crate::sio::Sio;
use crate::sounds::SoundPlayer;
use crate::timezone::TimezoneManager;
use crate::wifi::{AnnouncerFactory, SystemWifiHost, WifiManager};
use crate::zeroconf::{Announcer, MdnsResponder, ZeroConfAnnouncement};

pub struct App {
    pub env: Env,
    pub log: Arc<LogHooks>,
    pub config: Arc<ConfigStore>,
    pub sio: Arc<Sio>,
    pub sounds: Arc<SoundPlayer>,
    pub notifications: Arc<NotificationManager>,
    pub alarms: Arc<AlarmManager>,
    pub hostname: Arc<HostnameManager>,
    pub timezone: Arc<TimezoneManager>,
    pub wifi: Arc<WifiManager>,
    pub gatt: Arc<GattServer>,
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
        Arc::new(App {
            env,
            log,
            config,
            sio,
            sounds,
            notifications,
            alarms,
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
        let responder = MdnsResponder::new().map_err(|e| PyError::new("OSError", e))?;
        let source = Box::new(move || match wifi.upgrade() {
            Some(wifi) => wifi.get_current_config(),
            None => Err(PyError::new("RuntimeError", "the Wi-Fi manager is gone")),
        });
        let announcer: Arc<dyn Announcer> = Arc::new(ZeroConfAnnouncement::new(
            source,
            Arc::new(responder),
            ZeroConfAnnouncement::port_from_env(),
        ));
        Ok(announcer)
    })
}
