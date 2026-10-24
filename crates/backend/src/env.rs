//! Process configuration read once at start from the environment.
//!
//! Every variable and default is the Python's (`os.getenv(...)` across the
//! modules), so the systemd unit, `run_emulated.sh` and the test harnesses
//! keep working unchanged.

use std::path::PathBuf;

/// How the backend reaches the ESP32 (`BACKEND` in `machine.py`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendMode {
    /// The machine's UART `/dev/ttymxc0` and the board's GPIO lines.
    Fika,
    /// An ESP32 dev board on `/dev/ttyUSB0`.
    Usb,
    /// A replayed shot on a pty, no hardware.
    Emulator,
}

#[derive(Debug, Clone)]
pub struct Paths {
    pub config_dir: PathBuf,
    pub log_dir: PathBuf,
    pub history: PathBuf,
    pub debug_history: PathBuf,
    pub motor_energy: PathBuf,
    pub alarms: PathBuf,
    pub profiles: PathBuf,
    pub pour_over_profiles: PathBuf,
    pub images: PathBuf,
    pub default_images: PathBuf,
    pub user_sounds: PathBuf,
    pub system_sounds: PathBuf,
    pub reports: PathBuf,
    pub user_db_migrations: PathBuf,
    pub device_uuid_cache: PathBuf,
    pub smoke_validation_state: PathBuf,
    pub smoke_validation_version: PathBuf,
    pub redaction_key: PathBuf,
    pub timezone_json: PathBuf,
    pub firmware: PathBuf,
    pub simple_profile: PathBuf,
    /// Where the package's read-only data lives (`/opt/meticulous-backend`).
    pub package_root: PathBuf,
    /// The host OS files the system services read and write.
    pub system: SystemPaths,
}

/// Fixed paths of the machine image the Python hardcodes (no environment
/// variable), gathered so tests can point them into a temporary directory.
#[derive(Debug, Clone)]
pub struct SystemPaths {
    /// `ota.BUILD_DATE_FILE`.
    pub image_build_date: PathBuf,
    /// `ota.REPO_INFO_FILE`.
    pub image_summary: PathBuf,
    /// `ota.BUILD_CHANNEL_FILE`.
    pub image_build_channel: PathBuf,
    /// `ota.BUILD_VERSION_FILE`.
    pub image_build_version: PathBuf,
    /// `ota.HAWKBIT_CONFIG_DIR`: rauc-hawkbit-updater reads `channel` there.
    pub hawkbit_dir: PathBuf,
    /// `SSHManager.ISSUE_PATH`: the console banner that shows the root password.
    pub ssh_issue: PathBuf,
    /// `api.machine.FACTORY_RESET_ROOT`.
    pub factory_reset_root: PathBuf,
    /// The sysfs backlight device (`brightness`, `max_brightness`).
    pub backlight: PathBuf,
    /// `imager.PROVISION_DIR`.
    pub provisioning: PathBuf,
    /// `smoke_validation.PUBLISH_COMMAND`.
    pub smoke_publish_command: PathBuf,
    /// Where `DBusMonitor.notify_usb_test` mounts a USB stick.
    pub usb_test_mount: PathBuf,
}

impl SystemPaths {
    /// The machine's paths. `METICULOUS_ISSUE_PATH` is a test hook; with the
    /// hardware faked and no hook the banner goes next to the user data
    /// directory, where `tests/boot/boot_harness.py` puts it (`etc-issue`),
    /// and a factory reset wipes the user directory `CONFIG_PATH` is in
    /// rather than the host's `/meticulous-user`.
    fn machine(config_dir: &std::path::Path, fake_hardware: bool) -> SystemPaths {
        let user_dir = config_dir.parent().unwrap_or(config_dir);
        let ssh_issue = match var("METICULOUS_ISSUE_PATH") {
            Some(path) => PathBuf::from(path),
            None if fake_hardware => user_dir.parent().unwrap_or(user_dir).join("etc-issue"),
            None => PathBuf::from("/etc/issue"),
        };
        let factory_reset_root = if fake_hardware {
            user_dir.to_path_buf()
        } else {
            PathBuf::from("/meticulous-user")
        };
        SystemPaths {
            image_build_date: "/opt/ROOTFS_BUILD_DATE".into(),
            image_summary: "/opt/summary.txt".into(),
            image_build_channel: "/opt/image-build-channel".into(),
            image_build_version: "/opt/image-build-version".into(),
            hawkbit_dir: "/etc/hawkbit/".into(),
            ssh_issue,
            factory_reset_root,
            backlight: "/sys/class/backlight/backlight".into(),
            provisioning: "/meticulous-user/provisioning/".into(),
            smoke_publish_command: "/usr/local/bin/meticulous-smoke-report".into(),
            usb_test_mount: "/tmp/test_device".into(),
        }
    }

    #[cfg(test)]
    pub fn under(root: &std::path::Path) -> SystemPaths {
        SystemPaths {
            image_build_date: root.join("opt/ROOTFS_BUILD_DATE"),
            image_summary: root.join("opt/summary.txt"),
            image_build_channel: root.join("opt/image-build-channel"),
            image_build_version: root.join("opt/image-build-version"),
            hawkbit_dir: root.join("etc/hawkbit"),
            ssh_issue: root.join("etc/issue"),
            factory_reset_root: root.join("meticulous-user"),
            backlight: root.join("sys/class/backlight/backlight"),
            provisioning: root.join("meticulous-user/provisioning"),
            smoke_publish_command: root.join("usr/local/bin/meticulous-smoke-report"),
            usb_test_mount: root.join("tmp/test_device"),
        }
    }
}

impl Paths {
    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.yml")
    }

    pub fn database(&self) -> PathBuf {
        self.history.join("history.sqlite")
    }

    pub fn shots(&self) -> PathBuf {
        self.history.join("shots")
    }

    pub fn pour_over_history(&self) -> PathBuf {
        self.history.join("pour-over")
    }
}

/// Hooks for running the binary off a machine. None of them exist in the
/// Python, which its test harnesses replace in-process with mocks instead.
#[derive(Debug, Clone, Default)]
pub struct TestHooks {
    /// `METICULOUS_ESP_UART`: talk to this serial device (a pty, or fika-sil's
    /// `--pty-link`) instead of the board UART, without touching GPIO.
    pub esp_uart: Option<PathBuf>,
    /// `METICULOUS_FAKE_HARDWARE=1`: leave GPIO, I2C, BlueZ, systemd, the
    /// backlight, audio playback, disk imaging and the Wi-Fi manager alone,
    /// like `tests/boot/boot_harness.py` does to the Python.
    pub fake_hardware: bool,
}

#[derive(Debug, Clone)]
pub struct Env {
    pub backend: BackendMode,
    pub port: u16,
    pub debug: bool,
    /// Sentry is on for FIKA or when `SENTRY` is truthy (`back.py`).
    pub sentry: bool,
    pub log_level: String,
    pub force_stdout_log: bool,
    /// Percent of real time the emulator replays at (`EMULATION_SPEED`).
    pub emulation_speed: u32,
    pub paths: Paths,
    pub hooks: TestHooks,
}

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn path(name: &str, default: &str) -> PathBuf {
    PathBuf::from(var(name).unwrap_or_else(|| default.to_string()))
}

/// Python's `os.getenv(name, "False").lower() in ("true", "1", "y")`.
fn truthy(name: &str) -> bool {
    var(name).is_some_and(|v| matches!(v.to_lowercase().as_str(), "true" | "1" | "y"))
}

impl Env {
    pub fn from_process() -> Env {
        let backend = match var("BACKEND")
            .unwrap_or_else(|| "FIKA".to_string())
            .to_uppercase()
            .as_str()
        {
            "USB" => BackendMode::Usb,
            "EMULATOR" | "EMULATION" => BackendMode::Emulator,
            _ => BackendMode::Fika,
        };
        let package_root = path("METICULOUS_PACKAGE_ROOT", "/opt/meticulous-backend");
        let history = path("HISTORY_PATH", "/meticulous-user/history");
        let config_dir = path("CONFIG_PATH", "/meticulous-user/config");
        let fake_hardware = truthy("METICULOUS_FAKE_HARDWARE");
        let paths = Paths {
            system: SystemPaths::machine(&config_dir, fake_hardware),
            // smoke_validation.DEFAULT_STATE_FILE is CONFIG_PATH/smoke-validation.json.
            smoke_validation_state: var("SMOKE_VALIDATION_STATE_FILE")
                .map(PathBuf::from)
                .unwrap_or_else(|| config_dir.join("smoke-validation.json")),
            config_dir,
            log_dir: path("LOG_PATH", "/meticulous-user/logs"),
            debug_history: path("DEBUG_HISTORY_PATH", "/meticulous-user/history/debug"),
            history,
            motor_energy: path("MOTOR_ENERGY_PATH", "/meticulous-user/syslog/energy"),
            alarms: path("ALARMS_PATH", "/meticulous-user/syslog/alarms"),
            profiles: path("PROFILE_PATH", "/meticulous-user/profiles"),
            pour_over_profiles: path(
                "POUR_OVER_PROFILE_PATH",
                "/meticulous-user/pour-over-profiles",
            ),
            images: path("IMAGES_PATH", "/meticulous-user/profile-images/"),
            default_images: var("DEFAULT_IMAGES")
                .map(PathBuf::from)
                .unwrap_or_else(|| package_root.join("images/default")),
            user_sounds: path("USER_SOUNDS", "/meticulous-user/sounds"),
            system_sounds: var("SYSTEM_SOUNDS")
                .map(PathBuf::from)
                .unwrap_or_else(|| package_root.join("sounds")),
            reports: path("REPORTS_DIR", "/meticulous-user/reports"),
            user_db_migrations: path("USER_DB_MIGRATION_DIR", "/meticulous-user/.dbmigrations"),
            device_uuid_cache: path(
                "DEVICE_UUID_CACHE_PATH",
                "/meticulous-user/.device-identity/device-uuid",
            ),
            smoke_validation_version: path(
                "SMOKE_VALIDATION_VERSION_FILE",
                "/opt/image-build-version",
            ),
            redaction_key: path("REDACTION_KEY_PATH", "/root/.redaction_key"),
            timezone_json: path(
                "TIMEZONE_JSON_FILE_PATH",
                "/usr/share/zoneinfo/UI_timezones.json",
            ),
            firmware: path("UPDATE_PATH", "/opt/meticulous-firmware"),
            simple_profile: var("SIMPLE_PROFILE")
                .map(PathBuf::from)
                .unwrap_or_else(|| package_root.join("simple_profile.json")),
            package_root,
        };
        let hooks = TestHooks {
            esp_uart: var("METICULOUS_ESP_UART").map(PathBuf::from),
            fake_hardware,
        };
        Env {
            backend,
            port: var("PORT").and_then(|p| p.parse().ok()).unwrap_or(8080),
            debug: truthy("DEBUG"),
            sentry: backend == BackendMode::Fika || truthy("SENTRY"),
            log_level: var("LOGLEVEL")
                .unwrap_or_else(|| "DEBUG".to_string())
                .to_uppercase(),
            force_stdout_log: truthy("FORCE_STDOUT_LOG"),
            emulation_speed: var("EMULATION_SPEED")
                .and_then(|s| s.parse().ok())
                .unwrap_or(100),
            paths,
            hooks,
        }
    }

    pub fn emulated(&self) -> bool {
        self.backend == BackendMode::Emulator
    }

    /// Every writable path under `root`, read-only data from this checkout,
    /// no hardware: what `tests/boot/boot_harness.py` sets up for the Python.
    #[cfg(test)]
    pub fn for_tests(root: &std::path::Path, backend: BackendMode) -> Env {
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let user = root.join("meticulous-user");
        Env {
            backend,
            port: 0,
            debug: false,
            sentry: false,
            log_level: "DEBUG".into(),
            force_stdout_log: false,
            emulation_speed: 100,
            paths: Paths {
                config_dir: user.join("config"),
                log_dir: user.join("logs"),
                history: user.join("history"),
                debug_history: user.join("history/debug"),
                motor_energy: user.join("syslog/energy"),
                alarms: user.join("syslog/alarms"),
                profiles: user.join("profiles"),
                pour_over_profiles: user.join("pour-over-profiles"),
                images: user.join("profile-images"),
                default_images: repo.join("images/default"),
                user_sounds: user.join("sounds"),
                system_sounds: repo.join("sounds"),
                reports: user.join("reports"),
                user_db_migrations: user.join(".dbmigrations"),
                device_uuid_cache: user.join(".device-identity/device-uuid"),
                smoke_validation_state: user.join("smoke-validation.json"),
                smoke_validation_version: root.join("opt/image-build-version"),
                redaction_key: root.join("root/.redaction_key"),
                timezone_json: root.join("usr/share/zoneinfo/UI_timezones.json"),
                firmware: root.join("opt/meticulous-firmware"),
                simple_profile: repo.join("simple_profile.json"),
                package_root: repo,
                system: SystemPaths::under(root),
            },
            hooks: TestHooks {
                esp_uart: None,
                fake_hardware: true,
            },
        }
    }
}
