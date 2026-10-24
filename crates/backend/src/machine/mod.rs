//! `machine.py`: the ESP32 link, what the backend knows about the machine,
//! and the commands it sends.
//!
//! The Python keeps all of this in class attributes of `Machine`, written by
//! the UART thread and by request handlers at the same time. Here the state
//! sits behind one mutex that is only held while it is read or changed:
//! nothing that emits, plays a sound, saves a shot or writes to the UART runs
//! with it held. The reader thread (`reader.rs`) is the only consumer of the
//! port; writes from any thread go through the link's own lock, so the bytes
//! of one message are never interleaved with another's.

pub mod emulator;
pub mod link;
mod reader;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::Value;

use esp::encode;
use esp::sync::{DeviceUuidEnrollment, DeviceUuidStep, TareBehaviorPlan, TareBehaviorSync};
use esp::{
    EnergyCalculator, EspInfo, EspObservability, FirmwareVersion, HeaterTimeoutInfo, MachineStatus,
    SensorData, ShotData,
};

use crate::alarms::{AlarmType, minutes_left};
use crate::app::App;
use crate::config::keys;
use crate::env::BackendMode;
use crate::images::WARNING_TRIANGLE_IMAGE;
use crate::notifications::{Notification, now_seconds, responses};
use crate::sounds::SoundEvent;

use link::{EspLink, LinkKind};

/// The ESP does not always answer the mileage read issued with its first
/// info report; it is repeated at this interval until it does.
pub const MILEAGE_RETRY_INTERVAL_SECONDS: f64 = 5.0;
/// Until the ESP answered `action,info`, it is asked again at this interval.
pub const ESP_INFO_RETRY_INTERVAL_SECONDS: f64 = 2.0;
pub const DEVICE_UUID_RETRY_DELAY: Duration = Duration::from_secs(2);
const HAWKBIT_UPDATER_RESTART_TIMEOUT: Duration = Duration::from_secs(5);
/// Seconds of stable weight while retracting before a shot counts as over.
const STABLE_TIME_THRESHOLD: f64 = 2.0;

pub use esp::encode::{ALLOWED_BACKEND_ACTIONS, ALLOWED_ESP_ACTIONS};

/// The machine's last known state, as the rest of the backend reads it.
pub(crate) struct State {
    pub info_ready: bool,
    pub mileage: Option<i64>,
    mileage_seed: Option<i64>,
    mileage_last_request: f64,
    info_requested_at: Option<f64>,
    pub profile_ready: bool,
    old_profile_ready: bool,
    pub data_sensors: ShotData,
    pub sensor_sensors: Option<SensorData>,
    pub esp_info: Option<EspInfo>,
    reset_count: u32,
    pub firmware_available: Option<FirmwareVersion>,
    pub firmware_available_string: Option<String>,
    pub firmware_running: Option<FirmwareVersion>,
    start_time: f64,
    pub is_idle: bool,
    pub enable_manufacturing: bool,
    is_first_normal_boot: bool,
    stable_start_timestamp: Option<f64>,
    pub esp_restart_request: bool,
    pub observability: EspObservability,
    uuid: DeviceUuidEnrollment,
    tare: TareBehaviorSync,
    pub heater_timeout_info: Option<HeaterTimeoutInfo>,
    /// The notification the ESP's `Notify` messages update while the user
    /// has not acknowledged it (`Machine._espNotification`).
    esp_notification: Option<Notification>,
    esp_notification_shown: bool,
    // `_read_data`'s locals, kept across lines.
    old_status: String,
    time_flag: bool,
    shot_start_time: f64,
    time_passed: i64,
    profile_time: i64,
    push_to_brew: crate::shots::PushToBrewTimer,
    emulated_firmware: bool,
    previous_preheat_remaining: Option<f64>,
}

impl State {
    fn new(now_mono: f64) -> State {
        State {
            info_ready: false,
            mileage: None,
            mileage_seed: None,
            mileage_last_request: 0.0,
            info_requested_at: None,
            profile_ready: false,
            old_profile_ready: false,
            data_sensors: ShotData {
                state: MachineStatus::IDLE.to_string(),
                status: Some(MachineStatus::IDLE.to_string()),
                profile: Some(MachineStatus::IDLE.to_string()),
                ..ShotData::default()
            },
            sensor_sensors: None,
            esp_info: None,
            reset_count: 0,
            firmware_available: None,
            firmware_available_string: None,
            firmware_running: None,
            start_time: now_seconds(),
            is_idle: true,
            enable_manufacturing: false,
            is_first_normal_boot: false,
            stable_start_timestamp: None,
            esp_restart_request: false,
            observability: EspObservability::new(now_mono),
            uuid: DeviceUuidEnrollment::new(),
            tare: TareBehaviorSync::new(),
            heater_timeout_info: None,
            esp_notification: None,
            esp_notification_shown: false,
            old_status: MachineStatus::IDLE.to_string(),
            time_flag: false,
            shot_start_time: 0.0,
            time_passed: 0,
            profile_time: 0,
            push_to_brew: crate::shots::PushToBrewTimer::default(),
            emulated_firmware: false,
            previous_preheat_remaining: None,
        }
    }
}

pub struct Machine {
    pub emulated: bool,
    backend: BackendMode,
    link: Mutex<Option<EspLink>>,
    stop_comm: AtomicBool,
    pub(crate) state: Mutex<State>,
    pub(crate) energy: Mutex<EnergyCalculator>,
    /// Bumped whenever the device UUID retry timer is cancelled, so a timer
    /// that fires late does nothing.
    uuid_timer_generation: AtomicU64,
    clock: Instant,
}

impl Machine {
    pub fn new(backend: BackendMode) -> Machine {
        let clock = Instant::now();
        Machine {
            emulated: backend == BackendMode::Emulator,
            backend,
            link: Mutex::new(None),
            stop_comm: AtomicBool::new(false),
            state: Mutex::new(State::new(0.0)),
            energy: Mutex::new(EnergyCalculator::motor()),
            uuid_timer_generation: AtomicU64::new(0),
            clock,
        }
    }

    /// `time.monotonic()` for the observability state machine.
    pub fn monotonic(&self) -> f64 {
        self.clock.elapsed().as_secs_f64()
    }

    /// `Machine.init(sio)`: firmware on disk, serial number, manufacturing
    /// mode, the link, the greeting, and the reader thread. `Err` is what
    /// the Python raises, which ends the process.
    pub fn init(&self, app: &Arc<App>) -> Result<(), String> {
        {
            let mut state = self.state.lock();
            state.esp_restart_request = true;
            state.observability = EspObservability::new(self.monotonic());
        }
        self.restore_motor_energy(app);
        self.refresh_available_firmware(app);

        let serial = app.config.get(keys::SYSTEM, keys::MACHINE_SERIAL_NUMBER);
        if serial.is_null() || serial.as_str() == Some("") {
            let generated = generate_random_serial();
            app.config.set_and_save(
                keys::SYSTEM,
                keys::MACHINE_SERIAL_NUMBER,
                Value::String(generated),
            );
        }
        self.validate_manufacturing(app);

        if self.link.lock().is_some() {
            tracing::warn!("Machine.init was called twice!");
            return Ok(());
        }
        let mut emulator_control = None;
        let kind = match (&app.env.hooks.esp_uart, self.backend) {
            (Some(device), _) => LinkKind::Device(device.clone()),
            (None, BackendMode::Usb) => LinkKind::Usb,
            (None, BackendMode::Emulator) => {
                let (device, control) = emulator::start(app.env.emulation_speed)?;
                emulator_control = Some(control);
                LinkKind::Emulator(device)
            }
            (None, BackendMode::Fika) => LinkKind::Fika,
        };
        let mut link = EspLink::open(kind)?;
        link.emulator = emulator_control;
        let reader_port = link.reader()?;
        *self.link.lock() = Some(link);
        self.greet();

        if !self.emulated && app.env.hooks.esp_uart.is_none() {
            match som_revision() {
                Ok(som) => tracing::info!("SOM revision: {som}"),
                Err(error) => tracing::error!("Could not read the SOM revision: {error}"),
            }
        }

        let reader_app = app.clone();
        std::thread::Builder::new()
            .name("MachineSerial".into())
            .spawn(move || reader::run(reader_app, reader_port))
            .map_err(|e| e.to_string())?;

        let alive_app = app.clone();
        std::thread::Builder::new()
            .name("FlashingEsp".into())
            .spawn(move || {
                std::thread::sleep(Duration::from_secs(60));
                alive_app.machine.check_machine_alive(&alive_app);
            })
            .map_err(|e| e.to_string())?;

        self.start_motor_energy_saver(app);

        if self.state.lock().is_first_normal_boot {
            self.on_first_normal_boot(app);
        }
        Ok(())
    }

    /// `motor_energy_calculator` reads its saved total at import and saves it
    /// every second.
    fn restore_motor_energy(&self, app: &App) {
        let path = app
            .env
            .paths
            .motor_energy
            .join(esp::motor::MOTOR_ENERGY_FILE_NAME);
        let Ok(saved) = std::fs::read_to_string(&path) else {
            return;
        };
        let age = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let outcome = self.energy.lock().restore(&saved, age, self.monotonic());
        tracing::debug!("motor energy restored: {outcome:?}");
    }

    fn start_motor_energy_saver(&self, app: &Arc<App>) {
        let app = app.clone();
        let _ = std::thread::Builder::new()
            .name("save motor energy".into())
            .spawn(move || {
                tracing::info!("save_data_thread started");
                let dir = app.env.paths.motor_energy.clone();
                let path = dir.join(esp::motor::MOTOR_ENERGY_FILE_NAME);
                loop {
                    let text = app.machine.energy.lock().saved_text();
                    if let Err(error) =
                        std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, text))
                    {
                        tracing::debug!("cannot save motor energy: {error}");
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            });
    }

    fn on_first_normal_boot(&self, app: &App) {
        app.ssh.set_ssh_state(false);
        app.config
            .set_and_save(keys::USER, keys::SSH_ENABLED, Value::Bool(false));
    }

    pub fn toggle_manufacturing_mode(&self, app: &App, enabled: bool) {
        self.state.lock().enable_manufacturing = enabled;
        if enabled {
            tracing::info!("Sentry disabled: In Manufacturing mode");
        } else {
            tracing::info!("Sentry enabled: Manufacturing mode disabled");
        }
        app.reporting.set_paused(enabled);
        app.config.set_and_save(
            keys::MANUFACTURING,
            keys::LAST_BOOT_MODE,
            Value::String(if enabled { "manufacturing" } else { "normal" }.into()),
        );
    }

    /// `validate_manufacturing`: a machine without a real serial number is
    /// in manufacturing mode.
    fn validate_manufacturing(&self, app: &App) {
        if app
            .config
            .get_bool(keys::MANUFACTURING, keys::FORCE_MANUFACTURING_ENABLED)
        {
            self.toggle_manufacturing_mode(app, true);
            return;
        }
        let mut serial = app.config.get(keys::SYSTEM, keys::MACHINE_SERIAL_NUMBER);
        // Some older machines stored the serial as a number.
        if let Value::Number(n) = &serial {
            serial = Value::String(n.to_string());
            app.config
                .set_and_save(keys::SYSTEM, keys::MACHINE_SERIAL_NUMBER, serial.clone());
        }
        if let Some(serial) = serial.as_str() {
            if !serial.is_empty() && serial != "NOT_ASSIGNED" && !serial.starts_with("999") {
                let last_mode = app
                    .config
                    .get_str(keys::MANUFACTURING, keys::LAST_BOOT_MODE);
                self.state.lock().is_first_normal_boot =
                    last_mode.as_deref() == Some("manufacturing");
                return;
            }
        }
        self.toggle_manufacturing_mode(app, true);
    }

    pub fn enable_manufacturing(&self) -> bool {
        self.state.lock().enable_manufacturing
    }

    /// `check_machine_alive`, a minute after start.
    pub fn check_machine_alive(&self, app: &App) {
        if self.state.lock().info_ready {
            tracing::info!("The ESP is alive");
        } else if app
            .config
            .get_bool(keys::USER, keys::DISALLOW_FIRMWARE_FLASHING)
        {
            tracing::warn!("The ESP never send an info, but user requested no updates!");
        } else {
            tracing::warn!("The ESP never send an info, flashing latest firmware to be sure");
            self.start_update(app);
        }
    }

    /// `refreshAvailableFirmware`.
    pub fn refresh_available_firmware(&self, app: &App) -> Option<FirmwareVersion> {
        let text = crate::firmware::firmware_version(&app.env.paths.firmware);
        let parsed = esp::parse_version_string(text.as_deref());
        tracing::info!(
            "Backend available firmware version: {}",
            parsed
                .as_ref()
                .map(|v| format!("{v:?}"))
                .unwrap_or_else(|| "None".into())
        );
        let mut state = self.state.lock();
        state.firmware_available_string = text;
        state.firmware_available = parsed.clone();
        parsed
    }

    /// `_greet`: drop what the port buffered before it was opened, end any
    /// partial message the ESP holds, ask for its info.
    fn greet(&self) {
        if let Some(link) = self.link.lock().as_mut() {
            link.reset_input_buffer();
        }
        let [terminator, _info] = encode::greeting();
        self.write(&terminator);
        self.action_unchecked("info");
        self.state.lock().info_requested_at = Some(self.monotonic());
    }

    /// `Machine.write`: dropped while the ESP is being flashed.
    pub fn write(&self, bytes: &[u8]) {
        if self.stop_comm.load(Ordering::Relaxed) {
            return;
        }
        if let Some(link) = self.link.lock().as_mut() {
            if let Err(error) = link.write_all(bytes) {
                tracing::warn!("Writing to the ESP32 failed: {error}");
            }
        }
    }

    fn connected(&self) -> bool {
        !self.stop_comm.load(Ordering::Relaxed) && self.link.lock().is_some()
    }

    fn action_unchecked(&self, action: &str) {
        tracing::info!("sending action,{action}");
        self.write(&encode::action(action));
    }

    /// `Machine.action`: refused while the motor rests; `start` without a
    /// loaded profile first sends the last one.
    pub fn action(&self, app: &App, action: &str) -> bool {
        let alarm = app.alarms.end_time(AlarmType::MotorStressed);
        if action == "purge" {
            if let Some(end) = alarm {
                let warning =
                    format!("refusing action {action}, there is an alarm up (motor_stressed)");
                tracing::warn!("{warning}");
                app.reporting
                    .capture_message(&warning, sentry::Level::Warning);
                app.alarms.notify(
                    &format!(
                        "Brewing has been disabled because of a recent high strain on the motor, let it rest for {} more minutes",
                        minutes_left(end)
                    ),
                    WARNING_TRIANGLE_IMAGE,
                );
                return false;
            }
        }
        tracing::info!("sending action,{action}");
        if action == "start" && !self.state.lock().profile_ready {
            tracing::warn!("No profile loaded, sending last loaded profile to esp32");
            let Some(last) = app.profiles.last_profile() else {
                tracing::error!("No known last profile which could be sent to the esp32");
                return false;
            };
            let profile = last.get("profile").cloned().unwrap_or(Value::Null);
            if let Err(error) = app.profiles.send_profile_to_esp32(app, profile) {
                tracing::warn!("Could not send the last profile: {error}");
            }
        }
        if action == "home" || action == "purge" {
            self.state.lock().profile_ready = true;
        }
        self.write(&encode::action(action));
        true
    }

    /// `end_profile`: stop a running shot the way the stage allows.
    pub fn end_profile(&self, app: &App) {
        let (status, state) = {
            let s = self.state.lock();
            (
                s.data_sensors.status.clone().unwrap_or_default(),
                s.data_sensors.state.clone(),
            )
        };
        if status == "idle" {
            return;
        }
        tracing::info!("Ending profile due to user request");
        let user_stage = [
            "heating",
            "Pour water and click to continue",
            "click to start",
            "purge",
        ];
        if state == "brewing" && !user_stage.contains(&status.as_str()) {
            self.action(app, "home");
        } else {
            self.action(app, "stop");
        }
        app.sounds.play_event(SoundEvent::Abort);
    }

    /// `Machine.reset`.
    pub fn reset(&self) {
        {
            let mut state = self.state.lock();
            state.esp_restart_request = true;
            let now = self.monotonic();
            state.observability.begin_expected_reset(now);
            state.info_ready = false;
            state.profile_ready = false;
            state.start_time = now_seconds();
        }
        if let Some(link) = self.link.lock().as_mut() {
            link.reset(false);
        }
    }

    /// `send_json_with_hash`: stream a profile with the hash the firmware
    /// checks it against, then mark it loaded for the debug shot.
    pub fn send_json_with_hash(&self, app: &App, profile: &Value) {
        let upload = encode::send_json_with_hash(profile);
        tracing::debug!("streaming JSON to the machine");
        tracing::info!("JSON Hash: {}", upload.md5_hex);
        let started = Instant::now();
        self.write(&upload.bytes);
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        if ms > 10.0 {
            tracing::info!("Streaming profile to ESP32 took {} ms", ms as u64);
        } else {
            tracing::info!(
                "Streaming profile to ESP32 took {} ns",
                (ms * 1000.0) as u64
            );
        }
        self.state.lock().profile_ready = true;
        // Python spins until the reader thread opened the debug shot; here the
        // debug shot keeps the program until it starts.
        app.debug_shots.set_node_json(profile.clone());
    }

    /// `startScaleMasterCalibration`.
    pub fn start_scale_master_calibration(&self, app: &App) {
        self.action(app, "scale_master_calibration");
    }

    /// The scale calibration command with the current weight.
    pub fn calibrate(&self) {
        let weight = self.state.lock().data_sensors.weight.clone();
        self.write(&encode::calibration(&weight));
    }

    /// `HeaterActuator.set_timeout`.
    pub fn set_heater_timeout(&self, minutes: i64) -> Result<(), String> {
        let bytes = encode::heater_timeout(minutes)
            .map_err(|_| "Timeout must be between 0 and 60 minutes".to_string())?;
        tracing::info!("Setting heater timeout on ESP32: {minutes} minutes");
        self.write(&bytes);
        Ok(())
    }

    /// `startUpdate`: flash the firmware in `UPDATE_PATH`; the error text
    /// when it failed.
    pub fn start_update(&self, app: &App) -> Option<String> {
        let (available, previous) = {
            let s = self.state.lock();
            (
                s.firmware_available_string.clone(),
                s.esp_info.as_ref().map(|i| i.firmware_v.clone()),
            )
        };
        let now = self.monotonic();
        let diagnostics = self.state.lock().observability.begin_update(
            available.as_deref(),
            previous.as_deref(),
            now,
        );
        self.report_esp_diagnostics(app, diagnostics);
        self.stop_comm.store(true, Ordering::Relaxed);
        self.state.lock().esp_restart_request = true;

        let result = {
            let mut link = self.link.lock();
            match link.as_mut() {
                None => Err("Cannot update firmare!".to_string()),
                Some(link) => flash(app, link),
            }
        };
        let error = match result {
            Ok(()) => {
                self.state
                    .lock()
                    .observability
                    .finish_flashing(self.monotonic());
                None
            }
            Err(error) => {
                let diagnostic = self.state.lock().observability.fail_flashing(
                    &error,
                    "flash",
                    self.monotonic(),
                );
                self.report_esp_diagnostics(app, vec![diagnostic]);
                Some(error)
            }
        };
        self.stop_comm.store(false, Ordering::Relaxed);
        {
            let mut state = self.state.lock();
            state.esp_restart_request = state.observability.phase.value() != "normal";
        }
        if let Some(error) = &error {
            app.notifications.add(Notification::with_responses(
                format!(
                    "Realtime core upgrade failed: {error}. The machine will ensure a good state on next start. If you encounter any errors please reach out to product support!"
                ),
                &[responses::OK],
            ));
        }
        error
    }

    /// `_report_esp_diagnostics`: log, send to the ESP's Sentry project,
    /// raise the matching alarm.
    pub(crate) fn report_esp_diagnostics(&self, app: &App, diagnostics: Vec<esp::EspDiagnostic>) {
        for diagnostic in diagnostics {
            let tags: Vec<(String, String)> = diagnostic
                .tags
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            tracing::warn!("{}: tags={}", diagnostic.title, python_dict_repr(&tags));
            let previous = self.state.lock().observability.previous_firmware.clone();
            let firmware = diagnostic.firmware_version(previous.as_deref());
            let context = Value::Object(diagnostic.context.clone());
            app.reporting.capture_esp_event(
                &diagnostic.title,
                crate::sentry_reporting::level_from_name(&diagnostic.level),
                Some(diagnostic.fingerprint.clone()),
                &tags,
                (!diagnostic.context.is_empty()).then_some(("esp-diagnostic", &context)),
                firmware.as_deref(),
            );
            match diagnostic.alarm() {
                Some(esp::DiagnosticAlarm::EspRestart) => {
                    if app.alarms.end_time(AlarmType::EspRestart).is_none() {
                        app.alarms.set(AlarmType::EspRestart, None, false, true);
                    }
                }
                Some(esp::DiagnosticAlarm::EspDisconnected) => {
                    if app.alarms.end_time(AlarmType::EspDisconnected).is_none() {
                        app.alarms.set(AlarmType::EspDisconnected, None, true, true);
                    }
                }
                None => {}
            }
        }
    }

    /// `_handleMileageRequest`: seed the ESP's counter, once, with the shot
    /// count the history holds.
    fn handle_mileage_request(&self, app: &App) {
        let seed = self.state.lock().mileage_seed;
        let seed = match seed {
            Some(seed) => seed,
            None => match app.history.statistics() {
                Ok(stats) => {
                    let seed = stats
                        .get("totalSavedShots")
                        .and_then(Value::as_i64)
                        .unwrap_or(0);
                    self.state.lock().mileage_seed = Some(seed);
                    seed
                }
                Err(error) => {
                    tracing::error!("Could not compute mileage seed: {error}");
                    return;
                }
            },
        };
        if !self.connected() {
            tracing::warn!("Cannot seed mileage because the serial connection is not ready");
            return;
        }
        self.write(&encode::mileage_seed_write(seed));
        tracing::info!("Seeding ESP mileage counter with {seed} shots");
    }

    /// `requestMileage`.
    pub fn request_mileage(&self) {
        if !self.connected() {
            return;
        }
        self.write(&encode::mileage_read());
        self.state.lock().mileage_last_request = now_seconds();
    }

    /// `_handleNvsResponse`.
    fn handle_nvs_response(&self, key: &str, value: &str) {
        match esp::sync::classify_nvs_response(key, value) {
            esp::sync::NvsReply::Ignored => {}
            esp::sync::NvsReply::MileageWriteRejected(status) => {
                tracing::warn!("ESP rejected the mileage write: {status}")
            }
            esp::sync::NvsReply::MileageWriteAccepted => {
                tracing::info!("ESP accepted the mileage seed");
                self.request_mileage();
            }
            esp::sync::NvsReply::Mileage(mileage) => {
                self.state.lock().mileage = Some(mileage);
                tracing::info!("ESP reported mileage: {mileage}");
            }
            esp::sync::NvsReply::MileageUnparseable(text) => {
                tracing::warn!("Unparseable mileage from ESP: {text}")
            }
        }
    }

    /// `setSerial`.
    pub fn set_serial(
        &self,
        app: &App,
        color: &str,
        serial: &str,
        batch_number: &str,
        build_date: &str,
    ) {
        for bytes in encode::serial_writes(color, serial, batch_number, build_date) {
            self.write(&bytes);
        }
        app.notifications.add(Notification::with_responses(
            format!(
                "\nSerial number: {serial}\n\nBatch number: {batch_number}\n\nColor: {color}\n\nBuild Date: {build_date}\n            "
            ),
            &[responses::OK],
        ));
        app.config.set(
            keys::SYSTEM,
            keys::MACHINE_SERIAL_NUMBER,
            Value::String(serial.into()),
        );
        app.config.set(
            keys::SYSTEM,
            keys::MACHINE_COLOR,
            Value::String(color.into()),
        );
        app.config.set(
            keys::SYSTEM,
            keys::MACHINE_BATCH_NUMBER,
            Value::String(batch_number.into()),
        );
        app.config.set(
            keys::SYSTEM,
            keys::MACHINE_BUILD_DATE,
            Value::String(build_date.into()),
        );
        app.config.save();
    }

    /// `syncDeviceUUID`, on every ESPInfo.
    fn sync_device_uuid(&self, app: &Arc<App>, uuid: &str, supported: bool) {
        let steps = self.state.lock().uuid.sync(uuid, supported, || {
            crate::device_identity::generate_device_uuid()
        });
        self.run_uuid_steps(app, steps);
    }

    fn handle_device_uuid_response(&self, app: &Arc<App>, response: &str) {
        let steps = self.state.lock().uuid.handle_response(response);
        self.run_uuid_steps(app, steps);
    }

    fn reset_device_uuid_enrollment(&self) {
        let steps = self.state.lock().uuid.reset();
        for step in steps {
            if step == DeviceUuidStep::CancelRetry {
                self.uuid_timer_generation.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn run_uuid_steps(&self, app: &Arc<App>, steps: Vec<DeviceUuidStep>) {
        for step in steps {
            match step {
                DeviceUuidStep::CancelRetry => {
                    self.uuid_timer_generation.fetch_add(1, Ordering::Relaxed);
                }
                DeviceUuidStep::Write(bytes) => self.write(&bytes),
                DeviceUuidStep::ScheduleRetry => {
                    let generation = self.uuid_timer_generation.load(Ordering::Relaxed);
                    let app = app.clone();
                    let _ = std::thread::Builder::new()
                        .name("DeviceUUIDRetry".into())
                        .spawn(move || {
                            std::thread::sleep(DEVICE_UUID_RETRY_DELAY);
                            let machine = &app.machine;
                            if machine.uuid_timer_generation.load(Ordering::Relaxed) == generation {
                                let steps = machine.state.lock().uuid.retry_fired();
                                machine.run_uuid_steps(&app, steps);
                            }
                        });
                }
                DeviceUuidStep::UpdateCache(uuid) => {
                    match crate::device_identity::update_device_uuid_cache(
                        &app.env.paths.device_uuid_cache,
                        &uuid,
                    ) {
                        Err(error) => {
                            tracing::error!("Failed to update OTA device UUID cache: {error}")
                        }
                        Ok(false) => {}
                        Ok(true) => {
                            tracing::info!("Updated OTA device UUID cache from ESP32");
                            if !self.emulated {
                                let _ = std::thread::Builder::new()
                                    .name("HawkbitRestart".into())
                                    .spawn(restart_hawkbit_updater);
                            }
                        }
                    }
                }
                DeviceUuidStep::Log(log) => {
                    let text = device_uuid_log_text(&log);
                    match log.level() {
                        "info" => tracing::info!("{text}"),
                        "warning" => tracing::warn!("{text}"),
                        _ => tracing::error!("{text}"),
                    }
                }
            }
        }
    }

    /// `setPartialRetraction`.
    pub fn set_partial_retraction(&self, value: f64) {
        let esp_value = self
            .state
            .lock()
            .esp_info
            .as_ref()
            .map(|i| i.partial_retraction);
        if esp::sync::partial_retraction_in_sync(esp_value, value) {
            return;
        }
        if !self.connected() {
            tracing::warn!(
                "Cannot sync partial_retraction to ESP32 because serial connection is not ready"
            );
            return;
        }
        self.write(&encode::partial_retraction_write(value));
        tracing::info!("Synced partial_retraction to ESP32: requested={value:.2}");
        if let Some(info) = self.state.lock().esp_info.as_mut() {
            info.partial_retraction = value;
        }
    }

    /// `setAutoPurgeAfterShot`.
    pub fn set_auto_purge_after_shot(&self, value: bool) {
        let esp_value = self
            .state
            .lock()
            .esp_info
            .as_ref()
            .map(|i| i.auto_purge_after_shot);
        if esp::sync::auto_purge_in_sync(esp_value, value) {
            return;
        }
        if !self.connected() {
            tracing::warn!(
                "Cannot sync auto_purge_after_shot to ESP32 because serial connection is not ready"
            );
            return;
        }
        self.write(&encode::auto_purge_write(value));
        tracing::info!(
            "Synced auto_purge_after_shot to ESP32: requested={}",
            if value { "True" } else { "False" }
        );
        if let Some(info) = self.state.lock().esp_info.as_mut() {
            info.auto_purge_after_shot = value;
        }
    }

    /// `setTareBehavior`.
    pub fn set_tare_behavior(&self, value: &str) {
        let connected = self.connected();
        let plan = {
            let mut state = self.state.lock();
            let current = state
                .esp_info
                .as_ref()
                .and_then(|i| i.tare_behavior.clone());
            state.tare.plan(current.as_deref(), value, connected)
        };
        match plan {
            TareBehaviorPlan::Deferred => {
                tracing::info!("Deferring tare_behavior sync until supported firmware is connected")
            }
            TareBehaviorPlan::InSync | TareBehaviorPlan::AlreadyPending => {}
            TareBehaviorPlan::NotConnected => {
                tracing::warn!(
                    "Cannot sync tare_behavior to ESP32 because serial connection is not ready"
                )
            }
            TareBehaviorPlan::Write(bytes) => {
                self.write(&bytes);
                tracing::info!("Synced tare_behavior to ESP32: requested={value}");
            }
        }
    }

    fn handle_tare_behavior_response(&self, status: &str) {
        let response = self.state.lock().tare.on_response(status);
        match response {
            esp::sync::TareBehaviorResponse::NoPendingWrite => {
                tracing::warn!("Received tare_behavior NVS response without a pending write")
            }
            esp::sync::TareBehaviorResponse::Rejected { requested, status } => tracing::error!(
                "ESP32 rejected tare_behavior NVS write: requested={requested}, status={status}"
            ),
            esp::sync::TareBehaviorResponse::Confirmed(value) => {
                if let Some(info) = self.state.lock().esp_info.as_mut() {
                    info.tare_behavior = Some(value.clone());
                }
                tracing::info!("ESP32 confirmed tare_behavior NVS write: {value}");
            }
        }
    }

    /// `stopMotorIfHot`: too much energy through the motor in the last ten
    /// minutes stops the shot and blocks brewing for ten minutes.
    fn stop_motor_if_hot(&self, app: &App, shot: &ShotData, sensors: &SensorData) {
        let energy = self.energy.lock().calculate_motor_energy(
            &sensors.motor_power,
            &sensors.motor_current,
            Some((&shot.flow, &shot.pressure)),
            self.monotonic(),
        );
        let Ok(energy) = energy else { return };
        if esp::motor::exceeds_max_energy(energy)
            && app.alarms.end_time(AlarmType::MotorStressed).is_none()
        {
            app.alarms.set(
                AlarmType::MotorStressed,
                Some(now_seconds() + 600.0),
                true,
                false,
            );
            let idle =
                self.state.lock().data_sensors.status.as_deref() == Some(MachineStatus::IDLE);
            if !idle {
                self.end_profile(app);
                self.action(app, "home");
            }
        }
    }

    // Read accessors for handlers and other services.

    pub fn is_idle(&self) -> bool {
        self.state.lock().is_idle
    }

    pub fn profile_ready(&self) -> bool {
        self.state.lock().profile_ready
    }

    pub fn data_sensors(&self) -> ShotData {
        self.state.lock().data_sensors.clone()
    }

    pub fn sensor_sensors(&self) -> Option<SensorData> {
        self.state.lock().sensor_sensors.clone()
    }

    pub fn esp_info(&self) -> Option<EspInfo> {
        self.state.lock().esp_info.clone()
    }

    pub fn info_ready(&self) -> bool {
        self.state.lock().info_ready
    }

    pub fn mileage(&self) -> Option<i64> {
        self.state.lock().mileage
    }
}

/// `serial.flasher.flash`: the board's bootloader reset, the flash, a reset
/// back into the firmware, the baud rate restored.
fn flash(app: &App, link: &mut EspLink) -> Result<(), String> {
    if app.env.hooks.esp_uart.is_some() {
        return Err("firmware updates are not available in the boot harness".into());
    }
    if link.kind != LinkKind::Fika && link.kind != LinkKind::Usb {
        tracing::info!("Emulated ESP32 cannot be updated");
        return Ok(());
    }
    link.reset(true);
    let result = crate::firmware::flash(link.port_mut(), &app.env.paths.firmware);
    // esptool cannot reset the Fika board, so it is always reset by hand.
    link.reset(false);
    link.set_baud_rate(link::BAUDRATE);
    result
}

/// `generate_random_serial`: `999` and five digits.
fn generate_random_serial() -> String {
    let digits: String = (0..5)
        .map(|_| char::from(b'0' + rand::random_range(0u8..10)))
        .collect();
    format!("999{digits}")
}

/// `get_somrev`: the SOM revision register.
fn som_revision() -> Result<String, String> {
    let output = std::process::Command::new("i2cget")
        .args(["-f", "-y", "0x0", "0x52", "0x1e"])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!("i2cget exited with {}", output.status));
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let value = i64::from_str_radix(raw.trim_start_matches("0x"), 16).map_err(|e| e.to_string())?;
    Ok(format!("{}.{}", ((value & 0xE0) >> 5) + 1, value & 0x1F))
}

fn restart_hawkbit_updater() {
    let mut child = match std::process::Command::new("systemctl")
        .args(["--no-block", "restart", "rauc-hawkbit-updater.service"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            tracing::error!(
                "Could not restart rauc-hawkbit-updater after updating device UUID cache: {error}"
            );
            return;
        }
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return,
            Ok(Some(status)) => {
                let mut stderr = String::new();
                if let Some(mut pipe) = child.stderr.take() {
                    use std::io::Read;
                    let _ = pipe.read_to_string(&mut stderr);
                }
                tracing::warn!(
                    "Could not restart rauc-hawkbit-updater after updating device UUID cache: exit={} stderr={:?}",
                    status.code().unwrap_or(-1),
                    stderr
                );
                return;
            }
            Ok(None) if started.elapsed() > HAWKBIT_UPDATER_RESTART_TIMEOUT => {
                let _ = child.kill();
                tracing::error!(
                    "Could not restart rauc-hawkbit-updater after updating device UUID cache: timed out"
                );
                return;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(error) => {
                tracing::error!(
                    "Could not restart rauc-hawkbit-updater after updating device UUID cache: {error}"
                );
                return;
            }
        }
    }
}

fn device_uuid_log_text(log: &esp::sync::DeviceUuidLog) -> String {
    use esp::sync::{DeviceUuidLog as L, DeviceUuidResponse as R};
    const MAX: u32 = esp::sync::DEVICE_UUID_MAX_ASSIGNMENT_ATTEMPTS;
    match log {
        L::ReportedInvalid(uuid) => format!("ESP32 reported an invalid device UUID: {:?}", uuid),
        L::AssignmentsExhausted => format!(
            "ESP32 device UUID assignment exhausted {MAX} attempts; the OTA identity cache will remain unchanged until ESPInfo confirms a valid UUID"
        ),
        L::Accepted(R::Success) => {
            "ESP32 device UUID assignment response: SUCCESS; awaiting ESPInfo confirmation".into()
        }
        L::Accepted(_) => "ESP32 device UUID assignment response: ALREADY_ASSIGNED; awaiting ESPInfo confirmation of the stored UUID".into(),
        L::WriteFailedNothingPending => "ESP32 device UUID assignment response: ERROR_WRITE_FAILED, but no same-boot assignment is pending; no retry scheduled".into(),
        L::WriteFailedExhausted => format!(
            "ESP32 device UUID assignment response: ERROR_WRITE_FAILED; exhausted {MAX} attempts and the OTA identity cache remains unchanged"
        ),
        L::WriteFailedRetryPending => {
            "ESP32 device UUID assignment response: ERROR_WRITE_FAILED; a bounded retry is already scheduled".into()
        }
        L::WriteFailedScheduling { attempt } => format!(
            "ESP32 device UUID assignment response: ERROR_WRITE_FAILED; scheduling attempt {attempt} of {MAX} in {:.1} seconds",
            DEVICE_UUID_RETRY_DELAY.as_secs_f64()
        ),
        L::Rejected(response) => format!(
            "ESP32 device UUID assignment response: {response}; the OTA identity cache remains unchanged"
        ),
        L::Unrecognized(response) => format!(
            "ESP32 device UUID assignment response: {response}; unrecognized response and no retry scheduled"
        ),
    }
}

/// `str(dict)` of string pairs, for log lines that print Python dicts.
fn python_dict_repr(pairs: &[(String, String)]) -> String {
    let entries: Vec<String> = pairs
        .iter()
        .map(|(k, v)| {
            format!(
                "{}: {}",
                crate::metconfig::py_repr(&Value::String(k.clone())),
                crate::metconfig::py_repr(&Value::String(v.clone()))
            )
        })
        .collect();
    format!("{{{}}}", entries.join(", "))
}
