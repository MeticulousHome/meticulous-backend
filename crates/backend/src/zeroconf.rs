//! `api/zeroconf_announcement.py`: announce the machine on the local network
//! as `<hostname>._http._tcp.local.` and `<hostname>._meticulous._tcp.local.`
//! (the apps browse the second), with its addresses in the TXT record.
//!
//! The Wi-Fi manager restarts the announcement whenever the network changes.

use std::net::IpAddr;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::py::PyResult;
use crate::wifi::WifiSystemConfig;

/// What the Wi-Fi manager calls on `WifiManager._zeroconf`.
pub trait Announcer: Send + Sync {
    fn start(&self) -> PyResult<()>;
    fn stop(&self) -> PyResult<()>;
    fn restart(&self) -> PyResult<()> {
        self.stop()?;
        self.start()
    }
}

pub type ConfigSource = Box<dyn Fn() -> PyResult<WifiSystemConfig> + Send + Sync>;

/// The two services of one announcement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSpec {
    pub service_type: &'static str,
    pub instance: String,
    pub server: String,
    pub addresses: Vec<IpAddr>,
    pub port: u16,
    pub properties: Vec<(String, String)>,
}

impl ServiceSpec {
    pub fn fullname(&self) -> String {
        format!("{}.{}", self.instance, self.service_type)
    }
}

/// `_createServiceConfig()`: nothing to announce while disconnected.
pub fn service_specs(config: &WifiSystemConfig, port: u16) -> Option<[ServiceSpec; 2]> {
    if !config.connected {
        tracing::info!("Not connected to a network, not starting zeroconf");
        return None;
    }
    let machine_name = config.hostname.clone();
    let ips: Vec<String> = config.ips.iter().map(|ip| ip.addr.to_string()).collect();
    // TXT values must be text, not lists.
    let properties = vec![
        ("server_name".to_string(), machine_name.clone()),
        ("ips".to_string(), ips.join(",")),
        ("domain".to_string(), config.domains.join(",")),
        ("machine_name".to_string(), machine_name.clone()),
    ];
    let spec = |service_type: &'static str| ServiceSpec {
        service_type,
        instance: machine_name.clone(),
        server: format!("{machine_name}.local."),
        addresses: config.ips.iter().map(|ip| ip.addr).collect(),
        port,
        properties: properties.clone(),
    };
    Some([spec("_http._tcp.local."), spec("_meticulous._tcp.local.")])
}

/// Where announcements go: mDNS on the machine, a recorder in tests.
pub trait Responder: Send + Sync {
    fn register(&self, spec: &ServiceSpec) -> Result<(), String>;
    fn unregister(&self, fullname: &str) -> Result<(), String>;
}

/// `ZeroConfAnnouncement`.
pub struct ZeroConfAnnouncement {
    config_source: ConfigSource,
    responder: Arc<dyn Responder>,
    port: u16,
    /// `(http_service_info, met_service_info)` while registered.
    registered: Mutex<(Option<ServiceSpec>, Option<ServiceSpec>)>,
}

impl ZeroConfAnnouncement {
    pub fn new(config_source: ConfigSource, responder: Arc<dyn Responder>, port: u16) -> Self {
        ZeroConfAnnouncement {
            config_source,
            responder,
            port,
            registered: Mutex::new((None, None)),
        }
    }

    /// `ZEROCONF_PORT`.
    pub fn port_from_env() -> u16 {
        std::env::var("ZEROCONF_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(80)
    }
}

impl Announcer for ZeroConfAnnouncement {
    fn start(&self) -> PyResult<()> {
        tracing::info!("Registering Service with zeroconf");
        // The Python reads the configuration twice: once here, once to build
        // the services.
        (self.config_source)()?;
        let config = (self.config_source)()?;
        let specs = service_specs(&config, self.port);
        let (http, met) = match specs {
            Some([http, met]) => (Some(http), Some(met)),
            None => (None, None),
        };
        // A disconnected start keeps what an earlier start registered.
        let mut registered = self.registered.lock();
        let http = http.or_else(|| registered.0.clone());
        let met = met.or_else(|| registered.1.clone());
        match &http {
            Some(spec) => match self.responder.register(spec) {
                Ok(()) => tracing::info!("zeroconf service http announced on port {}", self.port),
                Err(error) => tracing::warn!(
                    "zeroconf failed to start on port {} error='{error}'",
                    self.port
                ),
            },
            None => tracing::warn!("Could not fetch machine informations for http zeroconf"),
        }
        match &met {
            Some(spec) => match self.responder.register(spec) {
                Ok(()) => tracing::info!(
                    "zeroconf service meticulous announced on port {}",
                    self.port
                ),
                Err(error) => tracing::warn!(
                    "zeroconf failed to start on port {} error='{error}'",
                    self.port
                ),
            },
            None => tracing::warn!("Could not fetch machine informations for meticulous zeroconf"),
        }
        *registered = (http, met);
        Ok(())
    }

    fn stop(&self) -> PyResult<()> {
        let mut registered = self.registered.lock();
        if let Some(spec) = registered.1.take() {
            if let Err(error) = self.responder.unregister(&spec.fullname()) {
                tracing::warn!("zeroconf unregister failed: {error}");
            }
            tracing::info!("zeroconf meticulous stopped");
        }
        if let Some(spec) = registered.0.take() {
            if let Err(error) = self.responder.unregister(&spec.fullname()) {
                tracing::warn!("zeroconf unregister failed: {error}");
            }
            tracing::info!("zeroconf http stopped");
        }
        Ok(())
    }
}

/// mDNS on every interface, IPv4 and IPv6 (`InterfaceChoice.All`,
/// `IPVersion.All`).
pub struct MdnsResponder {
    daemon: mdns_sd::ServiceDaemon,
}

impl MdnsResponder {
    pub fn new() -> Result<MdnsResponder, String> {
        mdns_sd::ServiceDaemon::new()
            .map(|daemon| MdnsResponder { daemon })
            .map_err(|e| e.to_string())
    }
}

impl Responder for MdnsResponder {
    fn register(&self, spec: &ServiceSpec) -> Result<(), String> {
        let properties: Vec<(&str, &str)> = spec
            .properties
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let info = mdns_sd::ServiceInfo::new(
            spec.service_type,
            &spec.instance,
            &spec.server,
            spec.addresses.as_slice(),
            spec.port,
            properties.as_slice(),
        )
        .map_err(|e| e.to_string())?;
        self.daemon.register(info).map_err(|e| e.to_string())
    }

    fn unregister(&self, fullname: &str) -> Result<(), String> {
        self.daemon
            .unregister(fullname)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wifi::types::IpNet;

    #[derive(Default)]
    struct Recorder {
        registered: Mutex<Vec<ServiceSpec>>,
        unregistered: Mutex<Vec<String>>,
    }

    impl Responder for Recorder {
        fn register(&self, spec: &ServiceSpec) -> Result<(), String> {
            self.registered.lock().push(spec.clone());
            Ok(())
        }
        fn unregister(&self, fullname: &str) -> Result<(), String> {
            self.unregistered.lock().push(fullname.into());
            Ok(())
        }
    }

    fn connected() -> WifiSystemConfig {
        let mut config = WifiSystemConfig::disconnected("meticulousX-1".into());
        config.connected = true;
        config.ips = vec![
            IpNet::parse("192.168.1.42/24").expect("ip"),
            IpNet::parse("fe80::1/64").expect("ip"),
        ];
        config.domains = vec!["lan".into()];
        config
    }

    /// `_createServiceConfig`: both services, the hostname as instance and
    /// server, addresses and TXT properties in the Python's order.
    #[test]
    fn services_describe_the_machine() {
        let [http, met] = service_specs(&connected(), 80).expect("connected");
        assert_eq!(http.fullname(), "meticulousX-1._http._tcp.local.");
        assert_eq!(met.fullname(), "meticulousX-1._meticulous._tcp.local.");
        assert_eq!(met.server, "meticulousX-1.local.");
        assert_eq!(met.port, 80);
        assert_eq!(met.addresses.len(), 2);
        assert_eq!(
            met.properties,
            vec![
                ("server_name".to_string(), "meticulousX-1".to_string()),
                ("ips".to_string(), "192.168.1.42,fe80::1".to_string()),
                ("domain".to_string(), "lan".to_string()),
                ("machine_name".to_string(), "meticulousX-1".to_string()),
            ]
        );
        assert!(service_specs(&WifiSystemConfig::disconnected("x".into()), 80).is_none());
    }

    /// start registers both, stop unregisters both, restart does both.
    #[test]
    fn announce_and_withdraw() {
        let recorder = Arc::new(Recorder::default());
        let connected_now = Arc::new(parking_lot::Mutex::new(true));
        let flag = connected_now.clone();
        let announcement = ZeroConfAnnouncement::new(
            Box::new(move || {
                if *flag.lock() {
                    Ok(connected())
                } else {
                    Ok(WifiSystemConfig::disconnected("x".into()))
                }
            }),
            recorder.clone(),
            80,
        );
        announcement.start().expect("start");
        assert_eq!(recorder.registered.lock().len(), 2);
        announcement.restart().expect("restart");
        assert_eq!(
            *recorder.unregistered.lock(),
            vec![
                "meticulousX-1._meticulous._tcp.local.".to_string(),
                "meticulousX-1._http._tcp.local.".to_string(),
            ]
        );
        assert_eq!(recorder.registered.lock().len(), 4);
        *connected_now.lock() = false;
        announcement.restart().expect("restart while disconnected");
        assert_eq!(recorder.registered.lock().len(), 4);
    }
}
