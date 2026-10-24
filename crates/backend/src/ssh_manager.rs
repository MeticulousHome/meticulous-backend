//! `ssh_manager.py`: the SSH unit and the root password. Leaving
//! manufacturing generates a password, stores it in the settings, sets it
//! with `chpasswd` and prints it on the console banner (`/etc/issue`), so the
//! serial console shows the current root password at every boot.

use std::path::PathBuf;
use std::sync::Arc;

use regex::{NoExpand, Regex};
use serde_json::{Value, json};

use crate::config::{ConfigStore, keys};
use crate::host::Host;
use crate::system_services::SystemServices;

pub struct SshManager {
    config: Arc<ConfigStore>,
    host: Arc<Host>,
    services: Arc<SystemServices>,
    issue_path: PathBuf,
}

impl SshManager {
    pub fn new(
        config: Arc<ConfigStore>,
        host: Arc<Host>,
        services: Arc<SystemServices>,
        issue_path: PathBuf,
    ) -> SshManager {
        SshManager {
            config,
            host,
            services,
            issue_path,
        }
    }

    /// `SSHManager.set_ssh_state`, for the settings API and the first boot
    /// after manufacturing.
    pub async fn set_ssh_state(&self, enabled: bool) -> bool {
        self.services
            .set_service_state("ssh.service", enabled)
            .await
    }

    /// `SSHManager.get_root_password`.
    pub fn root_password(&self) -> Value {
        self.config.get(keys::SYSTEM, keys::ROOT_PASSWORD)
    }

    /// `SSHManager.init`. `enable_manufacturing` is `Machine.enable_manufacturing`
    /// after `Machine.init`.
    pub fn init(&self, enable_manufacturing: bool, emulated: bool) {
        let mut current = self.root_password();
        tracing::info!(
            "Initializing SSH manager... Manufactoring mode: {}. Password already set: {}",
            if enable_manufacturing {
                "True"
            } else {
                "False"
            },
            if current.is_null() { "False" } else { "True" }
        );
        if emulated {
            return;
        }
        if !enable_manufacturing && current.is_null() {
            tracing::info!("Detected exit from manufacturing mode and generating password");
            if self.generate_root_password() {
                tracing::warn!("Root password generated successfully");
                current = self.root_password();
            } else {
                tracing::error!("Root password generation failed");
            }
        }
        if !current.is_null() {
            let password = crate::ota::python_str(&current);
            self.update_issue_file(&password);
            tracing::info!("Updated /etc/issue with current root password");
            self.set_root_password(&password);
            tracing::info!("The root password matches the configuration");
        }
    }

    /// `generate_root_password`.
    fn generate_root_password(&self) -> bool {
        let password = match self.generate_random_password() {
            Ok(password) => password,
            Err(kind) => {
                tracing::error!("Error generating or setting root password ({kind})");
                return false;
            }
        };
        // PARITY: a failed chpasswd is logged, the password is stored anyway.
        self.set_root_password(&password);
        self.config
            .set(keys::SYSTEM, keys::ROOT_PASSWORD, json!(password));
        self.config.save();
        tracing::info!("Successfully generated and set new root password");
        true
    }

    /// `pwgen -s -B -1 9`: nine secure characters without ambiguous ones.
    /// The error is the Python exception's type name.
    fn generate_random_password(&self) -> Result<String, &'static str> {
        let argv = ["pwgen", "-s", "-B", "-1", "9"];
        match self.host.run(&argv) {
            Ok(output) if output.success() => {
                Ok(crate::ota::py_strip(&output.stdout_text()).to_string())
            }
            Ok(_) => Err("CalledProcessError"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err("FileNotFoundError"),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                Err("PermissionError")
            }
            Err(_) => Err("OSError"),
        }
    }

    /// `set_root_password`: `root:<password>` into `chpasswd`.
    fn set_root_password(&self, password: &str) -> bool {
        let input = format!("root:{password}");
        let failure = match self.host.run_with_input(&["chpasswd"], input.as_bytes()) {
            Ok(output) if output.success() => return true,
            Ok(_) => "CalledProcessError",
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => "FileNotFoundError",
            Err(_) => "OSError",
        };
        tracing::error!("Error setting the root password ({failure})");
        false
    }

    /// `update_issue_file`: replace the banner's password block, or append it.
    fn update_issue_file(&self, password: &str) -> bool {
        static BLOCK: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let block =
            BLOCK.get_or_init(|| Regex::new(r"\nRoot password: .+\n\n").expect("valid regex"));
        let password_info = format!("\nRoot password: {password}\n\n");
        let result = (|| -> std::io::Result<()> {
            let content = match std::fs::read(&self.issue_path) {
                Ok(bytes) => {
                    let text =
                        crate::ota::universal_newlines(&String::from_utf8(bytes).map_err(|e| {
                            std::io::Error::new(std::io::ErrorKind::InvalidData, e)
                        })?);
                    if block.is_match(&text) {
                        block
                            .replace_all(&text, NoExpand(&password_info))
                            .into_owned()
                    } else {
                        text + &password_info
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => password_info.clone(),
                Err(error) => return Err(error),
            };
            std::fs::write(&self.issue_path, content)
        })();
        match result {
            Ok(()) => {
                tracing::info!(
                    "Successfully updated {} with root password",
                    self.issue_path.display()
                );
                true
            }
            Err(error) => {
                tracing::error!("Error updating {} file: {error}", self.issue_path.display());
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::CommandOutput;
    use crate::host::fake::FakeRunner;

    fn manager(dir: &tempfile::TempDir) -> (SshManager, Arc<FakeRunner>, Arc<ConfigStore>) {
        let config = Arc::new(ConfigStore::open(
            dir.path().join("config.yml"),
            crate::config::defaults(),
        ));
        let host = Arc::new(Host::default());
        let runner = Arc::new(FakeRunner::default());
        host.replace(runner.clone());
        let services = Arc::new(SystemServices::new(config.clone(), true));
        let manager = SshManager::new(config.clone(), host, services, dir.path().join("issue"));
        (manager, runner, config)
    }

    /// The bench machine prints a fresh root password on its console at the
    /// first normal boot: generated, stored, set and written to the banner.
    #[test]
    fn first_normal_boot_generates_and_shows_a_password() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (ssh, runner, config) = manager(&dir);
        runner.answer(
            "pwgen",
            CommandOutput {
                status: Some(0),
                stdout: b"Ab3dEf7gH\n".to_vec(),
                stderr: Vec::new(),
            },
        );
        std::fs::write(dir.path().join("issue"), "Debian GNU/Linux 12 \\n \\l\n").expect("issue");

        ssh.init(false, false);

        assert_eq!(
            config.get(keys::SYSTEM, keys::ROOT_PASSWORD),
            json!("Ab3dEf7gH")
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("issue")).expect("issue"),
            "Debian GNU/Linux 12 \\n \\l\n\nRoot password: Ab3dEf7gH\n\n"
        );
        let calls = runner.calls.lock().clone();
        assert_eq!(
            calls
                .iter()
                .map(|(argv, _)| argv[0].as_str())
                .collect::<Vec<_>>(),
            vec!["pwgen", "chpasswd", "chpasswd"]
        );
        assert_eq!(calls[1].1.as_deref(), Some(&b"root:Ab3dEf7gH"[..]));

        // Every later boot rewrites the same block and sets the password again.
        ssh.init(false, false);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("issue")).expect("issue"),
            "Debian GNU/Linux 12 \\n \\l\n\nRoot password: Ab3dEf7gH\n\n"
        );
    }

    /// A changed password replaces the old block; without a banner one is
    /// created.
    #[test]
    fn issue_block_is_replaced_or_created() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (ssh, _runner, config) = manager(&dir);
        config.set(keys::SYSTEM, keys::ROOT_PASSWORD, json!("new$1pass"));
        ssh.init(true, false);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("issue")).expect("issue"),
            "\nRoot password: new$1pass\n\n"
        );
        std::fs::write(
            dir.path().join("issue"),
            "banner\r\n\nRoot password: old\n\ntail\n",
        )
        .expect("issue");
        ssh.init(true, false);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("issue")).expect("issue"),
            "banner\n\nRoot password: new$1pass\n\ntail\n"
        );
    }

    /// Manufacturing machines and emulated ones get no password.
    #[test]
    fn manufacturing_and_emulation_leave_the_password_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (ssh, runner, config) = manager(&dir);
        ssh.init(true, false);
        ssh.init(false, true);
        assert!(runner.calls.lock().is_empty());
        assert_eq!(config.get(keys::SYSTEM, keys::ROOT_PASSWORD), Value::Null);
        assert!(!dir.path().join("issue").exists());
    }

    /// Without pwgen nothing is stored.
    #[test]
    fn missing_pwgen_stores_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (ssh, runner, config) = manager(&dir);
        runner.missing("pwgen");
        ssh.init(false, false);
        assert_eq!(config.get(keys::SYSTEM, keys::ROOT_PASSWORD), Value::Null);
        assert!(!dir.path().join("issue").exists());
    }
}
