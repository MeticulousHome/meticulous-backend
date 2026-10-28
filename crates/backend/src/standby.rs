//! ErP standby (Regulation (EU) 2023/826): once nobody has used the machine
//! for `standby_timeout` minutes (0 turns it off), the ESP32 light-sleeps, the Bluetooth radio
//! and the backlight go off, and the SoC suspends to RAM. A button or the dial
//! (through the ESP32), the app's WoWLAN magic packet or the power key wake it.
//! The image's systemd-sleep hook (meticulous-machine,
//! `meticulous-standby`) arms those wake-up sources and the Wi-Fi power save.
//!
//! New in the Rust backend: the Python backend has no standby.

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::{Value, json};

use crate::app::App;
use crate::config::keys;
use crate::http::Request;

/// Minutes without activity before standby, out of the box.
pub const STANDBY_TIMEOUT_DEFAULT: i64 = 15;
/// ErP: the machine reaches networked standby within 20 minutes.
pub const STANDBY_TIMEOUT_MAX: i64 = 20;
pub const STANDBY_TIMEOUT_MIN: i64 = 1;
/// `standby_timeout` 0: the user turned power save off.
pub const STANDBY_TIMEOUT_OFF: i64 = 0;

/// How often the idle timer is looked at.
const CHECK_INTERVAL: Duration = Duration::from_secs(5);
/// `systemctl start suspend.target` returns once the SoC resumed; the clock
/// of this timeout stops while it is suspended.
const SUSPEND_TIMEOUT: Duration = Duration::from_secs(120);
/// Where the Dial posts its own dimming: not a person.
const BACKLIGHT_PATH: &str = "/api/v1/machine/backlight";

/// How long the ESP32 is waited for, and how often `action,wake` is repeated
/// (the bytes that wake it are lost, so the first one never arrives).
#[derive(Debug, Clone, Copy)]
pub struct Timings {
    pub esp_reply: Duration,
    pub esp_wake: Duration,
    pub wake_interval: Duration,
    pub poll: Duration,
}

impl Default for Timings {
    fn default() -> Timings {
        Timings {
            esp_reply: Duration::from_secs(3),
            esp_wake: Duration::from_secs(5),
            wake_interval: Duration::from_millis(250),
            poll: Duration::from_millis(50),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Awake,
    /// `action,standby` sent, waiting for the ESP32's answer.
    Requested,
    /// The ESP32 sleeps; the SoC is suspending or suspended.
    Asleep,
    /// The SoC resumed and is waking the ESP32.
    Waking,
}

pub struct Standby {
    last_activity: Mutex<Instant>,
    phase: Mutex<Phase>,
    timings: Timings,
}

impl Default for Standby {
    fn default() -> Standby {
        Standby::with_timings(Timings::default())
    }
}

impl Standby {
    pub fn with_timings(timings: Timings) -> Standby {
        Standby {
            last_activity: Mutex::new(Instant::now()),
            phase: Mutex::new(Phase::Awake),
            timings,
        }
    }

    /// Someone used the machine: the idle timer starts again.
    pub fn note_activity(&self) {
        *self.last_activity.lock() = Instant::now();
    }

    pub fn phase(&self) -> Phase {
        *self.phase.lock()
    }

    /// The ESP32 is silent on purpose: no link timeout may fire.
    pub fn esp_asleep(&self) -> bool {
        matches!(self.phase(), Phase::Asleep | Phase::Waking)
    }

    /// `Standby,<event>,<detail>` from the ESP32.
    pub fn on_esp_line(&self, event: &str, detail: &str) {
        let mut phase = self.phase.lock();
        match event {
            "entering" if *phase == Phase::Requested => *phase = Phase::Asleep,
            "refused" => {
                tracing::info!("The ESP32 refused standby: {detail}");
                *phase = Phase::Awake;
                drop(phase);
                self.note_activity();
            }
            "awake" => {
                tracing::info!("The ESP32 woke from standby: {detail}");
                *phase = Phase::Awake;
                drop(phase);
                self.note_activity();
            }
            _ => {}
        }
    }

    /// Valid traffic from the ESP32: once the SoC resumed, it is awake.
    pub fn on_esp_traffic(&self) {
        let mut phase = self.phase.lock();
        if *phase == Phase::Waking {
            *phase = Phase::Awake;
        }
    }

    /// A request counts as someone using the machine when it changes
    /// something (the Dial's own dimming aside) or comes from another
    /// device: the app or the web UI, through nginx.
    pub fn note_request(&self, req: &Request) {
        let changes = req.method != http::Method::GET && req.path != BACKLIGHT_PATH;
        if changes || !is_loopback(&client_ip(req)) {
            self.note_activity();
        }
    }

    /// Whether the machine should go to standby now, after `idle` without
    /// activity.
    pub fn due_after(&self, app: &App, idle: Duration) -> bool {
        if self.phase() != Phase::Awake {
            return false;
        }
        let minutes = app
            .config
            .get_i64(keys::USER, keys::STANDBY_TIMEOUT)
            .unwrap_or(STANDBY_TIMEOUT_DEFAULT);
        if minutes == STANDBY_TIMEOUT_OFF {
            return false;
        }
        let minutes = minutes.clamp(STANDBY_TIMEOUT_MIN, STANDBY_TIMEOUT_MAX);
        let timeout = Duration::from_secs(u64::try_from(minutes).unwrap_or(1) * 60);
        idle >= timeout && app.machine.ready_for_standby() && !app.os_update.in_progress()
    }

    fn due(&self, app: &App) -> bool {
        let idle = self.last_activity.lock().elapsed();
        self.due_after(app, idle)
    }

    /// Put the ESP32 and the SoC to sleep, and bring them back once the SoC
    /// resumed. Returns false when the ESP32 did not go to sleep.
    pub fn enter(&self, app: &Arc<App>, runtime: &tokio::runtime::Handle) -> bool {
        *self.phase.lock() = Phase::Requested;
        tracing::info!("No activity, entering standby");
        app.machine.action(app, "standby");
        if !self.wait_while(Phase::Requested, self.timings.esp_reply) {
            tracing::warn!("The ESP32 did not answer action,standby");
            *self.phase.lock() = Phase::Awake;
            self.note_activity();
            return false;
        }
        if self.phase() != Phase::Asleep {
            return false;
        }

        let leave_hardware_alone = app.machine.emulated || app.env.hooks.fake_hardware;
        app.backlight
            .dim(&json!(0), &Value::Null, &Value::Null, leave_hardware_alone);
        runtime.block_on(app.gatt.set_radio_powered(false));
        self.suspend(app);
        self.wake(app);
        runtime.block_on(app.gatt.set_radio_powered(true));
        true
    }

    /// Suspend to RAM through systemd directly rather than logind: logind's
    /// PrepareForSleep would have NetworkManager take Wi-Fi down, and the app
    /// could no longer wake the machine.
    fn suspend(&self, app: &App) {
        let argv = ["systemctl", "start", "suspend.target"];
        match app.host.run_with_timeout(&argv, SUSPEND_TIMEOUT) {
            Ok(output) if output.success() => tracing::info!("Resumed from standby"),
            Ok(output) => tracing::warn!(
                "Suspend failed: {}",
                crate::host::describe_failure(&argv, &output)
            ),
            Err(error) => tracing::warn!("Suspend failed: {error}"),
        }
    }

    /// Repeat `action,wake` until the ESP32 talks again (it may already be
    /// awake: a button or the dial woke both).
    fn wake(&self, app: &App) {
        {
            let mut phase = self.phase.lock();
            if *phase == Phase::Asleep {
                *phase = Phase::Waking;
            }
        }
        let started = Instant::now();
        while self.phase() == Phase::Waking && started.elapsed() < self.timings.esp_wake {
            app.machine.action(app, "wake");
            self.wait_while(Phase::Waking, self.timings.wake_interval);
        }
        if self.phase() == Phase::Waking {
            tracing::warn!("The ESP32 did not wake from standby");
            *self.phase.lock() = Phase::Awake;
        }
        self.note_activity();
    }

    /// Wait up to `timeout` for the phase to leave `phase`; true when it did.
    fn wait_while(&self, phase: Phase, timeout: Duration) -> bool {
        let started = Instant::now();
        while self.phase() == phase {
            if started.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(self.timings.poll);
        }
        true
    }
}

/// Start the idle timer, on real hardware only: the emulated ESP32 does not
/// sleep.
pub fn start(app: Arc<App>) {
    if app.emulated() || app.env.hooks.fake_hardware {
        return;
    }
    let runtime = tokio::runtime::Handle::current();
    let spawned = std::thread::Builder::new()
        .name("standby".into())
        .spawn(move || {
            loop {
                std::thread::sleep(CHECK_INTERVAL);
                if app.standby.due(&app) {
                    app.standby.enter(&app, &runtime);
                }
            }
        });
    if let Err(error) = spawned {
        tracing::error!("Could not start the standby timer: {error}");
    }
}

/// The settings handler's check for `standby_timeout`: off, or 1 to 20 minutes.
pub fn validate_timeout(value: &Value) -> Result<(), String> {
    match value.as_i64() {
        Some(STANDBY_TIMEOUT_OFF) => Ok(()),
        Some(minutes) if (STANDBY_TIMEOUT_MIN..=STANDBY_TIMEOUT_MAX).contains(&minutes) => Ok(()),
        _ => Err(format!(
            "standby_timeout must be {STANDBY_TIMEOUT_OFF} (off) or between {STANDBY_TIMEOUT_MIN} and {STANDBY_TIMEOUT_MAX} minutes"
        )),
    }
}

/// The client behind nginx (`X-Real-IP`), or the peer itself.
fn client_ip(req: &Request) -> String {
    req.headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| req.remote_ip.clone())
}

fn is_loopback(ip: &str) -> bool {
    ip.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    //! The machine goes to standby only when nobody uses it and it is safe,
    //! and comes back with the ESP32 talking: the ErP standby and networked
    //! standby limits depend on it, and a machine that sleeps mid-shot or
    //! never wakes its ESP32 reaches customers.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::host::fake::FakeRunner;
    use crate::os_update::OsStatus;
    use crate::testing;

    const FAST: Timings = Timings {
        esp_reply: Duration::from_millis(300),
        esp_wake: Duration::from_millis(300),
        wake_interval: Duration::from_millis(20),
        poll: Duration::from_millis(5),
    };
    const TIMEOUT: Duration = Duration::from_secs(STANDBY_TIMEOUT_DEFAULT as u64 * 60);

    fn ready_app() -> (tempfile::TempDir, Arc<App>) {
        let (dir, app) = testing::app();
        {
            let mut state = app.machine.state.lock();
            state.info_ready = true;
            state.is_idle = true;
        }
        (dir, app)
    }

    /// Only after `standby_timeout` minutes of an idle machine, and never
    /// when standby is off, a shot runs, the ESP32 is not ready or an update
    /// is being installed.
    #[test]
    fn standby_is_due_only_for_an_idle_machine_after_the_timeout() {
        let (_dir, app) = ready_app();
        let standby = Standby::with_timings(FAST);
        assert!(!standby.due_after(&app, TIMEOUT - Duration::from_secs(1)));
        assert!(standby.due_after(&app, TIMEOUT));

        app.machine.state.lock().time_flag = true;
        assert!(!standby.due_after(&app, TIMEOUT));
        app.machine.state.lock().time_flag = false;

        app.machine.state.lock().info_ready = false;
        assert!(!standby.due_after(&app, TIMEOUT));
        app.machine.state.lock().info_ready = true;

        app.os_update.set_last(OsStatus::Installing, 10.0);
        assert!(!standby.due_after(&app, TIMEOUT));
        app.os_update.set_last(OsStatus::Idle, 0.0);

        app.config.set_and_save(
            keys::USER,
            keys::STANDBY_TIMEOUT,
            json!(STANDBY_TIMEOUT_OFF),
        );
        assert!(!standby.due_after(&app, TIMEOUT * 4));
    }

    /// The ESP32 answers `Standby,entering`, the SoC suspends through
    /// systemd, and after resume the backend repeats `action,wake` until the
    /// ESP32 talks again.
    #[test]
    fn standby_sleeps_the_esp32_and_the_soc_then_wakes_the_esp32() {
        let (_dir, app) = ready_app();
        app.machine.record_writes();
        let runner = Arc::new(FakeRunner::default());
        app.host.replace(runner.clone());
        let standby = Arc::new(Standby::with_timings(FAST));
        let esp = {
            let (app, standby) = (app.clone(), standby.clone());
            std::thread::spawn(move || {
                wait_for_write(&app, "action,standby\x03");
                standby.on_esp_line("entering", "light_sleep");
                wait_for_write(&app, "action,wake\x03");
                standby.on_esp_traffic();
            })
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();

        assert!(standby.enter(&app, runtime.handle()));
        esp.join().unwrap();

        assert_eq!(runner.argvs(), [["systemctl", "start", "suspend.target"]]);
        let writes = app.machine.writes();
        assert_eq!(
            writes.first().map(Vec::as_slice),
            Some(&b"action,standby\x03"[..])
        );
        assert!(writes[1..].iter().all(|w| w == b"action,wake\x03") && writes.len() > 1);
        assert_eq!(standby.phase(), Phase::Awake);
        assert!(!standby.esp_asleep());
    }

    /// A refusal or no answer leaves the SoC awake.
    #[test]
    fn the_soc_stays_awake_when_the_esp32_refuses_or_does_not_answer() {
        let (_dir, app) = ready_app();
        app.machine.record_writes();
        let runner = Arc::new(FakeRunner::default());
        app.host.replace(runner.clone());
        let runtime = tokio::runtime::Runtime::new().unwrap();

        let standby = Arc::new(Standby::with_timings(FAST));
        let esp = {
            let (app, standby) = (app.clone(), standby.clone());
            std::thread::spawn(move || {
                wait_for_write(&app, "action,standby\x03");
                standby.on_esp_line("refused", "not idle");
            })
        };
        assert!(!standby.enter(&app, runtime.handle()));
        esp.join().unwrap();
        assert_eq!(standby.phase(), Phase::Awake);

        let silent = Standby::with_timings(FAST);
        assert!(!silent.enter(&app, runtime.handle()));
        assert_eq!(silent.phase(), Phase::Awake);
        assert!(runner.argvs().is_empty());
    }

    /// The Dial's own polling and dimming must not keep the machine awake;
    /// a change, or anything from the app or the web UI, must.
    #[tokio::test]
    async fn requests_count_as_activity_unless_they_are_the_dials_polling() {
        let (_dir, app) = testing::app();
        let idle = |app: &Arc<App>| app.standby.last_activity.lock().elapsed();
        let backdate = |app: &Arc<App>| {
            *app.standby.last_activity.lock() = Instant::now() - Duration::from_secs(600);
        };

        backdate(&app);
        testing::call(&app, http::Method::GET, "/api/v1/settings", "").await;
        assert!(idle(&app) >= Duration::from_secs(600));
        testing::call(
            &app,
            http::Method::POST,
            BACKLIGHT_PATH,
            r#"{"brightness": 0.3}"#,
        )
        .await;
        assert!(idle(&app) >= Duration::from_secs(600));

        testing::call(
            &app,
            http::Method::POST,
            "/api/v1/settings",
            r#"{"enable_sounds": false}"#,
        )
        .await;
        assert!(idle(&app) < Duration::from_secs(60));

        backdate(&app);
        testing::call_with_headers(
            &app,
            http::Method::GET,
            "/api/v1/settings",
            "",
            &[("X-Real-IP", "192.168.1.20")],
        )
        .await;
        assert!(idle(&app) < Duration::from_secs(60));
    }

    /// ErP caps the delay before networked standby at 20 minutes; 0 is the
    /// Dial's and the app's "Off".
    #[tokio::test]
    async fn standby_timeout_is_refused_outside_off_or_one_to_twenty_minutes() {
        let (_dir, app) = testing::app();
        for minutes in [-1, 21] {
            let reply = testing::call(
                &app,
                http::Method::POST,
                "/api/v1/settings",
                format!(r#"{{"standby_timeout": {minutes}}}"#),
            )
            .await;
            assert_eq!(reply.status_code(), 400, "{minutes} minutes");
        }
        let reply = testing::call(
            &app,
            http::Method::POST,
            "/api/v1/settings",
            r#"{"standby_timeout": 20}"#,
        )
        .await;
        assert_eq!(reply.status_code(), 200);
        assert_eq!(
            app.config.get_i64(keys::USER, keys::STANDBY_TIMEOUT),
            Some(20)
        );
        let reply = testing::call(
            &app,
            http::Method::POST,
            "/api/v1/settings",
            r#"{"standby_timeout": 0}"#,
        )
        .await;
        assert_eq!(reply.status_code(), 200);
        assert_eq!(
            app.config.get_i64(keys::USER, keys::STANDBY_TIMEOUT),
            Some(STANDBY_TIMEOUT_OFF)
        );
    }

    fn wait_for_write(app: &App, message: &str) {
        let started = Instant::now();
        while !app
            .machine
            .writes()
            .iter()
            .any(|w| w.as_slice() == message.as_bytes())
        {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "no {message:?} written"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
