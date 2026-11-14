//! `system_services.py`: enable or disable systemd units over D-Bus. The
//! settings decide whether SSH runs; at boot the unit follows the settings.

use std::sync::Arc;

use futures::future::BoxFuture;
use parking_lot::RwLock;

use crate::config::{ConfigStore, keys};

/// One call on `org.freedesktop.systemd1.Manager`, as `pydbus` makes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemdCall {
    EnableUnitFiles(Vec<String>, bool, bool),
    StartUnit(String, String),
    StopUnit(String, String),
    DisableUnitFiles(Vec<String>, bool),
    Reload,
}

/// systemd's manager; tests and the faked hardware record the calls.
pub trait Systemd: Send + Sync {
    fn call(&self, call: SystemdCall) -> BoxFuture<'_, Result<(), String>>;
}

/// systemd on the system bus, connected on first use (`SystemBus()`).
#[derive(Default)]
pub struct ZbusSystemd {
    connection: tokio::sync::OnceCell<zbus::Connection>,
}

const SYSTEMD_NAME: &str = "org.freedesktop.systemd1";
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
const SYSTEMD_MANAGER: &str = "org.freedesktop.systemd1.Manager";

impl ZbusSystemd {
    async fn connection(&self) -> Result<&zbus::Connection, String> {
        self.connection
            .get_or_try_init(zbus::Connection::system)
            .await
            .map_err(|e| e.to_string())
    }

    async fn invoke(&self, call: SystemdCall) -> Result<(), String> {
        let connection = self.connection().await?;
        let result = match &call {
            SystemdCall::EnableUnitFiles(files, runtime, force) => {
                connection
                    .call_method(
                        Some(SYSTEMD_NAME),
                        SYSTEMD_PATH,
                        Some(SYSTEMD_MANAGER),
                        "EnableUnitFiles",
                        &(files, *runtime, *force),
                    )
                    .await
            }
            SystemdCall::StartUnit(name, mode) => {
                connection
                    .call_method(
                        Some(SYSTEMD_NAME),
                        SYSTEMD_PATH,
                        Some(SYSTEMD_MANAGER),
                        "StartUnit",
                        &(name, mode),
                    )
                    .await
            }
            SystemdCall::StopUnit(name, mode) => {
                connection
                    .call_method(
                        Some(SYSTEMD_NAME),
                        SYSTEMD_PATH,
                        Some(SYSTEMD_MANAGER),
                        "StopUnit",
                        &(name, mode),
                    )
                    .await
            }
            SystemdCall::DisableUnitFiles(files, runtime) => {
                connection
                    .call_method(
                        Some(SYSTEMD_NAME),
                        SYSTEMD_PATH,
                        Some(SYSTEMD_MANAGER),
                        "DisableUnitFiles",
                        &(files, *runtime),
                    )
                    .await
            }
            SystemdCall::Reload => {
                connection
                    .call_method(
                        Some(SYSTEMD_NAME),
                        SYSTEMD_PATH,
                        Some(SYSTEMD_MANAGER),
                        "Reload",
                        &(),
                    )
                    .await
            }
        };
        result.map(|_| ()).map_err(|e| e.to_string())
    }
}

impl Systemd for ZbusSystemd {
    fn call(&self, call: SystemdCall) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(self.invoke(call))
    }
}

/// Accepts every call, like the `MagicMock` the boot harness puts in place
/// of `system_services.SystemBus`.
#[derive(Default)]
pub struct RecordingSystemd {
    pub calls: parking_lot::Mutex<Vec<SystemdCall>>,
    /// Fail every call with this message (a bus that is not there).
    pub failure: Option<String>,
}

impl Systemd for RecordingSystemd {
    fn call(&self, call: SystemdCall) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            if let Some(failure) = &self.failure {
                return Err(failure.clone());
            }
            self.calls.lock().push(call);
            Ok(())
        })
    }
}

pub struct SystemServices {
    config: Arc<ConfigStore>,
    systemd: RwLock<Arc<dyn Systemd>>,
}

impl SystemServices {
    pub fn new(config: Arc<ConfigStore>, fake_hardware: bool) -> SystemServices {
        let systemd: Arc<dyn Systemd> = if fake_hardware {
            Arc::new(RecordingSystemd::default())
        } else {
            Arc::new(ZbusSystemd::default())
        };
        SystemServices {
            config,
            systemd: RwLock::new(systemd),
        }
    }

    #[cfg(test)]
    pub fn replace_systemd(&self, systemd: Arc<dyn Systemd>) {
        *self.systemd.write() = systemd;
    }

    /// `SystemServices.set_service_state`: enable and start, or stop and
    /// disable, then reload. `false` (logged) when any step fails.
    pub async fn set_service_state(&self, service: &str, enabled: bool) -> bool {
        let systemd = self.systemd.read().clone();
        let unit = vec![service.to_string()];
        let steps = if enabled {
            vec![
                SystemdCall::EnableUnitFiles(unit, false, false),
                SystemdCall::StartUnit(service.to_string(), "fail".into()),
            ]
        } else {
            vec![
                SystemdCall::StopUnit(service.to_string(), "fail".into()),
                SystemdCall::DisableUnitFiles(unit, false),
            ]
        };
        for step in steps {
            if let Err(error) = systemd.call(step).await {
                tracing::error!("Error while managing {service}: {error}");
                return false;
            }
        }
        if enabled {
            tracing::info!("{service} enabled and started");
        } else {
            tracing::info!("{service} stopped and disabled");
        }
        if let Err(error) = systemd.call(SystemdCall::Reload).await {
            tracing::error!("Error while managing {service}: {error}");
            return false;
        }
        true
    }

    /// `SystemServices.init`: the settings decide whether SSH runs.
    pub async fn init(&self, emulated: bool) {
        if emulated {
            tracing::info!("Skipping system services init in emulated mode");
            return;
        }
        let ssh_enabled = if self.config.contains(keys::USER, keys::SSH_ENABLED) {
            self.config.get_bool(keys::USER, keys::SSH_ENABLED)
        } else {
            true
        };
        tracing::info!(
            "Syncing ssh.service state: {}",
            if ssh_enabled { "enabled" } else { "disabled" }
        );
        self.set_service_state("ssh.service", ssh_enabled).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn services() -> (tempfile::TempDir, SystemServices, Arc<RecordingSystemd>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = Arc::new(ConfigStore::open(
            dir.path().join("config.yml"),
            crate::config::defaults(),
        ));
        let services = SystemServices::new(config, true);
        let systemd = Arc::new(RecordingSystemd::default());
        services.replace_systemd(systemd.clone());
        (dir, services, systemd)
    }

    /// tests/test_api_settings.py::test_post_ssh_disabled_stops_and_disables_the_unit
    #[tokio::test]
    async fn disabling_stops_and_disables_the_unit() {
        let (_dir, services, systemd) = services();
        assert!(services.set_service_state("ssh.service", false).await);
        assert_eq!(
            *systemd.calls.lock(),
            vec![
                SystemdCall::StopUnit("ssh.service".into(), "fail".into()),
                SystemdCall::DisableUnitFiles(vec!["ssh.service".into()], false),
                SystemdCall::Reload,
            ]
        );
    }

    /// tests/test_api_settings.py::test_post_ssh_enabled_enables_and_starts_the_unit
    #[tokio::test]
    async fn enabling_enables_and_starts_the_unit() {
        let (_dir, services, systemd) = services();
        assert!(services.set_service_state("ssh.service", true).await);
        assert_eq!(
            *systemd.calls.lock(),
            vec![
                SystemdCall::EnableUnitFiles(vec!["ssh.service".into()], false, false),
                SystemdCall::StartUnit("ssh.service".into(), "fail".into()),
                SystemdCall::Reload,
            ]
        );
    }

    /// tests/test_api_settings.py::test_post_ssh_failure_returns_500_and_keeps_the_old_value:
    /// without a bus the state change reports failure.
    #[tokio::test]
    async fn unreachable_bus_reports_failure() {
        let (_dir, services, _systemd) = services();
        services.replace_systemd(Arc::new(RecordingSystemd {
            failure: Some("D-Bus unavailable".into()),
            ..RecordingSystemd::default()
        }));
        assert!(!services.set_service_state("ssh.service", false).await);
    }

    /// SystemServices.init: emulated machines are left alone; otherwise the
    /// unit follows `ssh_enabled`.
    #[tokio::test]
    async fn init_follows_the_settings() {
        let (_dir, services, systemd) = services();
        services.init(true).await;
        assert!(systemd.calls.lock().is_empty());
        services
            .config
            .set(keys::USER, keys::SSH_ENABLED, serde_json::json!(false));
        services.init(false).await;
        assert_eq!(
            systemd.calls.lock().first(),
            Some(&SystemdCall::StopUnit("ssh.service".into(), "fail".into()))
        );
    }
}
