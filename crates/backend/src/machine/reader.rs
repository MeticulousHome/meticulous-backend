//! `Machine._read_data`: every line from the ESP32, in the Python's order of
//! checks and effects.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use serialport::SerialPort;

use esp::{ButtonEvent, ClassifiedLine, EspLine, MachineStatus};

use crate::app::App;
use crate::config::keys;
use crate::notifications::{Notification, now_seconds, responses};
use crate::sounds::SoundEvent;

use super::link::LineReader;
use super::{
    ESP_INFO_RETRY_INTERVAL_SECONDS, MILEAGE_RETRY_INTERVAL_SECONDS, STABLE_TIME_THRESHOLD,
};

const READ_TIMEOUT: Duration = Duration::from_millis(500);

pub(super) fn run(app: Arc<App>, port: Box<dyn SerialPort>) {
    let machine = &app.machine;
    let mut reader = LineReader::new(port);
    {
        let mut state = machine.state.lock();
        state.shot_start_time = now_seconds();
        state.start_time = now_seconds();
    }
    tracing::info!("Starting to listen for esp32 messages");
    loop {
        if machine.stop_comm.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(100));
            machine.state.lock().start_time = now_seconds();
            continue;
        }
        let valid = match reader.read_line(READ_TIMEOUT) {
            Some(bytes) if !bytes.is_empty() => match esp::decode_line(&bytes) {
                Some(text) => handle_line(&app, text),
                None => {
                    tracing::info!("decoding fails, message: {}", python_bytes_repr(&bytes));
                    // The Python's `continue` skips the timeout check below.
                    continue;
                }
            },
            _ => None,
        };
        let now = machine.monotonic();
        if let Some(message_type) = valid {
            let diagnostics = {
                let mut state = machine.state.lock();
                let firmware = message_type.1.clone();
                state
                    .observability
                    .observe_valid_message(message_type.0, now, firmware.as_deref())
            };
            machine.report_esp_diagnostics(&app, diagnostics);
            {
                let mut state = machine.state.lock();
                state.esp_restart_request = state.observability.phase.value() != "normal";
                state.reset_count = 0;
            }
            app.alarms.clear(crate::alarms::AlarmType::EspDisconnected);
            app.alarms.clear(crate::alarms::AlarmType::EspRestart);
        }
        let diagnostics = machine.state.lock().observability.check_timeouts(now);
        machine.report_esp_diagnostics(&app, diagnostics);
    }
}

/// One decoded line. Returns the valid message type (and the firmware
/// version an ESPInfo carried) when the line counts as valid traffic.
fn handle_line(app: &Arc<App>, data_str: &str) -> Option<(&'static str, Option<String>)> {
    let machine = &app.machine;
    if app
        .config
        .get_bool(keys::LOGGING, keys::LOGGING_SENSOR_MESSAGES)
    {
        tracing::info!("{}", pyfmt::strip_crlf(data_str));
    }
    let now = machine.monotonic();
    let classified = ClassifiedLine::new(data_str);
    let is_boot_banner = classified.boot_banner;
    if is_boot_banner {
        let diagnostics = machine
            .state
            .lock()
            .observability
            .observe_raw_line(data_str, now);
        machine.report_esp_diagnostics(app, diagnostics);
    }

    let mut is_valid_message = true;
    if is_boot_banner {
        {
            let mut state = machine.state.lock();
            state.reset_count += 1;
            state.start_time = now_seconds();
            state.esp_info = None;
            state.tare.clear();
            state.info_requested_at = None;
            state.info_ready = false;
            state.profile_ready = false;
        }
        machine.reset_device_uuid_enrollment();
        is_valid_message = false;
    }

    let (reset_count, update_in_progress) = {
        let state = machine.state.lock();
        (state.reset_count, state.observability.update_in_progress())
    };
    if reset_count >= 3 && !update_in_progress {
        tracing::warn!("The ESP seems to be resetting, sending update now");
        machine.start_update(app);
        machine.state.lock().reset_count = 0;
    }

    let ask_info = {
        let state = machine.state.lock();
        state.info_ready
            && state.esp_info.is_none()
            && state
                .info_requested_at
                .is_none_or(|at| machine.monotonic() - at >= ESP_INFO_RETRY_INTERVAL_SECONDS)
    };
    if ask_info {
        tracing::info!("Machine has not provided us with a firmware version yet. Requesting now");
        machine.action(app, "info");
        machine.state.lock().info_requested_at = Some(machine.monotonic());
    }

    let mut valid_message_type: Option<&'static str> = None;
    let mut button = None;
    let mut sensor = None;
    let mut data = None;
    let mut info = None;
    let mut notify = None;
    let mut boot_reason = None;

    match classified.line {
        EspLine::Event(event) => {
            button = event;
            valid_message_type = Some("Event");
        }
        EspLine::Data(parsed) => {
            data = parsed;
            valid_message_type = Some("Data");
        }
        EspLine::Sensors(parsed) => {
            sensor = parsed;
            valid_message_type = Some("Sensors");
        }
        EspLine::EspInfo(parsed) => {
            info = parsed;
            valid_message_type = Some("ESPInfo");
        }
        EspLine::EspBoot { reason, code } => {
            boot_reason = Some((reason, code));
            valid_message_type = Some("ESPBoot");
        }
        EspLine::DeviceUuidResponse(response) => {
            machine.handle_device_uuid_response(app, &response);
            valid_message_type = Some("DeviceUUIDResponse");
        }
        EspLine::TareBehaviorNvsResponse(status) => {
            machine.handle_tare_behavior_response(&status);
            valid_message_type = Some("NVSResponse");
        }
        EspLine::Notify(parsed) => {
            notify = Some(parsed);
            valid_message_type = Some("Notify");
        }
        EspLine::HeaterTimeoutInfo(result) => {
            valid_message_type = Some("HeaterTimeoutInfo");
            match result {
                Ok(heater) => {
                    let remaining = heater.preheat_remaining;
                    let previous = {
                        let mut state = machine.state.lock();
                        state.heater_timeout_info = Some(heater);
                        state.previous_preheat_remaining.replace(remaining)
                    };
                    app.sio.emit("heater_status", &serde_json::json!(remaining));
                    if remaining == 0.0 && previous != Some(0.0) {
                        tracing::info!("Heater_status: off");
                    }
                }
                Err(error) => tracing::error!("Error processing HeaterTimeoutInfo: {error}"),
            }
        }
        EspLine::Log(result) => {
            valid_message_type = Some("Log");
            match result {
                Ok(log) => handle_esp_log(app, log),
                Err(error) => tracing::error!("Error processing ESP log ({error})"),
            }
        }
        EspLine::MileageRequest => {
            machine.handle_mileage_request(app);
            valid_message_type = Some("MileageRequest");
        }
        EspLine::NvsResponse { key, value } => {
            machine.handle_nvs_response(&key, &value);
            valid_message_type = Some("NVSResponse");
        }
        EspLine::NvsError(text) => {
            tracing::warn!("ESP nvs_response error: {text}");
            valid_message_type = Some("NVSResponse");
        }
        EspLine::Unknown => {
            tracing::info!("{}", pyfmt::strip_crlf(data_str));
            is_valid_message = false;
        }
        EspLine::Uncaught(error) => {
            // Deliberate fix: Python raises here and the exception ends the
            // UART thread for good. The line is dropped instead.
            tracing::error!("Dropped a malformed ESP32 line: {error}");
            is_valid_message = false;
        }
    }

    if (!is_valid_message || valid_message_type.is_none()) && !is_boot_banner {
        let diagnostics = machine
            .state
            .lock()
            .observability
            .observe_raw_line(data_str, now);
        machine.report_esp_diagnostics(app, diagnostics);
    }
    if let Some((reason, code)) = &boot_reason {
        let diagnostics = machine
            .state
            .lock()
            .observability
            .observe_boot_reason(reason, code, now);
        machine.report_esp_diagnostics(app, diagnostics);
    }

    let mileage_due = {
        let state = machine.state.lock();
        state.info_ready
            && state.mileage.is_none()
            && now_seconds() - state.mileage_last_request >= MILEAGE_RETRY_INTERVAL_SECONDS
    };
    if mileage_due {
        machine.request_mileage();
    }

    let old_ready = machine.state.lock().info_ready;

    if let Some(data) = data {
        handle_shot_data(app, data);
    }
    if let Some(sensor) = sensor {
        let (shot, time_flag) = {
            let mut state = machine.state.lock();
            state.sensor_sensors = Some(sensor.clone());
            (state.data_sensors.clone(), state.time_flag)
        };
        machine.stop_motor_if_hot(app, &shot, &sensor);
        app.debug_shots.handle_sensor_data(&sensor);
        app.debug_shots.handle_shot_data(app, &shot);
        if time_flag {
            app.shots.handle_sensor_data(&sensor);
            app.shots.handle_shot_data(app, &shot);
        }
    }
    let info_firmware = info.as_ref().map(|i| i.firmware_v.clone());
    if let Some(info) = info {
        handle_esp_info(app, info);
    }
    if let Some(button) = button {
        if button.event != ButtonEvent::EncoderClockwise
            && button.event != ButtonEvent::EncoderCounterclockwise
        {
            tracing::debug!("Button Event recieved: {button:?}");
        }
        app.sio.emit("button", &button.to_sio());
    }
    let (now_ready, idle) = {
        let state = machine.state.lock();
        (
            state.info_ready,
            state.data_sensors.status.as_deref() == Some(MachineStatus::IDLE),
        )
    };
    if !old_ready
        && now_ready
        && app.config.get_bool(keys::USER, keys::MACHINE_HEAT_ON_BOOT)
        && idle
    {
        tracing::info!("Tell the machine to preheat");
        tracing::warn!("NOT IMPLEMENTED YET");
    }
    if let Some(notify) = notify {
        handle_notify(app, notify);
    }

    match valid_message_type {
        Some(kind) if is_valid_message => Some((kind, info_firmware)),
        _ => None,
    }
}

/// What the data block does besides changing the state, in the order the
/// Python does it. Collected under the state lock, run after it is released.
enum Effect {
    ShotStart(i64),
    ShotStop,
    Sound(SoundEvent),
    RequestMileage,
    DebugStart,
    DebugStop,
}

/// The `if data is not None` block: shot start and end, sounds, debug shots,
/// the timing every status carries.
fn handle_shot_data(app: &Arc<App>, data: esp::ShotData) {
    let machine = &app.machine;
    let status = data.status.clone().unwrap_or_default();
    let is_idle = status == MachineStatus::IDLE;
    let is_purge = status == MachineStatus::PURGE;
    let is_retracting = status == MachineStatus::RETRACTING;
    let is_preparing = status == MachineStatus::CLOSING_VALVE;
    let is_heating = status == MachineStatus::HEATING;
    let is_starting = status == MachineStatus::STARTING;

    let mut effects = Vec::new();
    let (time_flag, retracting_extraction) = {
        let mut state = machine.state.lock();
        state.is_idle = is_idle;
        let old_status = state.old_status.clone();
        let was_preparing = old_status == MachineStatus::CLOSING_VALVE;
        if old_status == MachineStatus::IDLE && !is_idle {
            state.push_to_brew.reset();
        }
        state.push_to_brew.observe(&status, machine.monotonic());

        if was_preparing && status != old_status {
            state.time_flag = true;
            state.shot_start_time = now_seconds();
            tracing::info!("shot start_time: {:.1}", state.shot_start_time);
            effects.push(Effect::ShotStart(state.push_to_brew.duration_ms));
            effects.push(Effect::Sound(SoundEvent::BrewingStart));
        } else if state.time_flag {
            if is_idle || is_purge {
                state.time_flag = false;
            }
            // After retracting the shot is always over.
            if old_status == MachineStatus::RETRACTING && !is_retracting {
                state.time_flag = false;
                effects.push(Effect::ShotStop);
            }
            // While retracting, the shot ends once the weight held still.
            if is_retracting {
                match state.stable_start_timestamp {
                    Some(start) => {
                        state.time_flag = now_seconds() - start < STABLE_TIME_THRESHOLD;
                        if !data.stable_weight {
                            state.stable_start_timestamp = None;
                        }
                    }
                    None => {
                        if data.stable_weight {
                            state.stable_start_timestamp = Some(now_seconds());
                        }
                    }
                }
            }
            if !state.time_flag {
                let now = now_seconds();
                match state.stable_start_timestamp.take() {
                    Some(start) => tracing::info!(
                        "shot ended at {} with a stable weight time of: {}s",
                        pyfmt::repr_f64(now),
                        pyfmt::repr_f64(now - start)
                    ),
                    None => tracing::info!("shot ended with weight unstable"),
                }
                effects.push(Effect::Sound(SoundEvent::BrewingEnd));
                effects.push(Effect::ShotStop);
                // The ESP counted this shot: re-read its counter.
                effects.push(Effect::RequestMileage);
            }
        }

        if is_idle && old_status != MachineStatus::IDLE {
            state.profile_ready = false;
        }
        if old_status == MachineStatus::IDLE
            && !is_idle
            && (is_heating || is_preparing || is_retracting || is_starting)
        {
            state.time_passed = 0;
            state.profile_time = 0;
        }
        if state.profile_ready && !state.old_profile_ready {
            effects.push(Effect::DebugStart);
        }
        if !state.profile_ready && state.old_profile_ready {
            effects.push(Effect::DebugStop);
        }
        state.old_profile_ready = state.profile_ready;
        if is_idle && old_status != MachineStatus::IDLE {
            effects.push(Effect::Sound(SoundEvent::Idle));
        }
        if is_heating && old_status != MachineStatus::HEATING {
            state.time_passed = 0;
            state.profile_time = 0;
            effects.push(Effect::Sound(SoundEvent::HeatingStart));
        }
        if old_status == MachineStatus::HEATING && !is_heating {
            effects.push(Effect::Sound(SoundEvent::HeatingEnd));
        }
        if state.time_flag {
            state.time_passed = ((now_seconds() - state.shot_start_time) * 1000.0) as i64;
            state.profile_time = state.time_passed;
        }
        (state.time_flag, state.time_flag && is_retracting)
    };

    for effect in effects {
        match effect {
            Effect::ShotStart(push_to_brew_ms) => app.shots.start(push_to_brew_ms),
            Effect::ShotStop => app.shots.stop(app),
            Effect::Sound(sound) => {
                app.sounds.play_event(sound);
            }
            Effect::RequestMileage => machine.request_mileage(),
            Effect::DebugStart => app.debug_shots.start(app),
            Effect::DebugStop => app.debug_shots.stop(app),
        }
    }

    let (time_passed, mut profile_time) = {
        let state = machine.state.lock();
        (state.time_passed, state.profile_time)
    };
    if retracting_extraction {
        profile_time = app.shots.handle_extraction_end(time_passed);
    }
    let new_status = {
        let mut state = machine.state.lock();
        state.profile_time = profile_time;
        state.data_sensors = data.clone_with_time_and_state(time_passed, time_flag, profile_time);
        state.old_status = state.data_sensors.status.clone().unwrap_or_default();
        state.info_ready = true;
        state.old_status.clone()
    };
    app.smoke.observe_status(&new_status, time_flag);
}

/// The `if info is not None` block.
fn handle_esp_info(app: &Arc<App>, info: esp::EspInfo) {
    let machine = &app.machine;
    let running = esp::parse_version_string(Some(&info.firmware_v));
    {
        let mut state = machine.state.lock();
        state.esp_info = Some(info.clone());
        state.info_ready = true;
    }
    machine.request_mileage();
    machine.state.lock().firmware_running = running.clone();

    let retraction = app
        .config
        .get_f64(keys::USER, keys::PROFILE_PARTIAL_RETRACTION)
        .unwrap_or(crate::config::PARTIAL_RETRACTION_DEFAULT);
    machine.set_partial_retraction(retraction);
    machine.set_auto_purge_after_shot(app.config.get_bool(keys::USER, keys::PROFILE_AUTO_PURGE));
    let tare = match app.config.get(keys::USER, keys::PROFILE_TARE_BEHAVIOR) {
        Value::String(s) => s,
        other => crate::metconfig::py_repr(&other),
    };
    machine.set_tare_behavior(&tare);
    machine.sync_device_uuid(app, &info.device_uuid, info.device_uuid_supported);

    let assigned = |v: &str| !v.is_empty() && v != "NOT_ASSIGNED";
    if assigned(&info.serial_number)
        && assigned(&info.color)
        && assigned(&info.batch_number)
        && assigned(&info.build_date)
    {
        app.config.set(
            keys::SYSTEM,
            keys::MACHINE_SERIAL_NUMBER,
            Value::String(info.serial_number.clone()),
        );
        app.config.set(
            keys::SYSTEM,
            keys::MACHINE_COLOR,
            Value::String(info.color.clone()),
        );
        app.config.set(
            keys::SYSTEM,
            keys::MACHINE_BATCH_NUMBER,
            Value::String(info.batch_number.clone()),
        );
        app.config.set(
            keys::SYSTEM,
            keys::MACHINE_BUILD_DATE,
            Value::String(info.build_date.clone()),
        );
        app.config.save();
    }

    // Enable / disable manufacturing mode based on the ESP's answer.
    let serial_assigned = !app
        .config
        .get(keys::SYSTEM, keys::MACHINE_SERIAL_NUMBER)
        .is_null();
    if machine.enable_manufacturing() != serial_assigned
        && !app
            .config
            .get_bool(keys::MANUFACTURING, keys::FORCE_MANUFACTURING_ENABLED)
    {
        machine.toggle_manufacturing_mode(app, false);
    }

    let (emulated_firmware, available) = {
        let state = machine.state.lock();
        (state.emulated_firmware, state.firmware_available.clone())
    };
    if !emulated_firmware {
        tracing::info!(
            "ESPInfo running firmware version:   {} on pinout version {} Machine_color: {} Serial_number: {} Batch_number: {} Build_date: {}",
            running
                .as_ref()
                .map(|v| format!("{v:?}"))
                .unwrap_or_else(|| "None".into()),
            info.esp_pinout,
            info.color,
            info.serial_number,
            info.batch_number,
            info.build_date
        );
        tracing::info!(
            "Backend available firmware version: {}",
            available
                .as_ref()
                .map(|v| format!("{v:?}"))
                .unwrap_or_else(|| "None".into())
        );
        machine.state.lock().emulated_firmware = machine.emulated;
    }
    let update_in_progress = machine.state.lock().observability.update_in_progress();
    if esp::should_start_firmware_update(
        available.as_ref(),
        running.as_ref(),
        update_in_progress,
        app.config
            .get_bool(keys::USER, keys::DISALLOW_FIRMWARE_FLASHING),
    ) {
        match &running {
            Some(running) => tracing::info!(
                "Firmware {}-{} is outdated, upgrading",
                running.release,
                running.extra_commits
            ),
            // Deliberate fix: Python raises AttributeError here, ending the
            // UART thread, when the running version did not parse.
            None => tracing::info!("Firmware of unknown version is outdated, upgrading"),
        }
        machine.start_update(app);
    }
}

/// The `case ["Log", *log_data]` branch.
fn handle_esp_log(app: &Arc<App>, log: esp::EspLog) {
    match log.level.as_str() {
        "debug" => tracing::debug!(target: "meticulous_backend::machine", "{}", log.full_message),
        "info" => tracing::info!(target: "meticulous_backend::machine", "{}", log.full_message),
        "warning" => tracing::warn!(target: "meticulous_backend::machine", "{}", log.full_message),
        // Logged at ERROR, which also reports it to the backend's project.
        "error" => {
            tracing::error!(target: "meticulous_backend::machine", "ESP error: {}", log.message)
        }
        _ => {}
    }
    for item in &log.malformed_items {
        tracing::warn!("Error parsing ESP log item: {item}");
    }
    app.smoke.observe_esp_log(&log.message);
    if log.send_to_sentry {
        let firmware = {
            let state = app.machine.state.lock();
            match (&state.esp_info, &state.firmware_running) {
                (Some(info), Some(_)) => {
                    Some(info.firmware_v.trim().to_string()).filter(|v| !v.is_empty())
                }
                _ => None,
            }
        };
        let context = log.items.as_ref().map(|items| {
            Value::Object(
                items
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                    .collect(),
            )
        });
        app.reporting.capture_esp_event(
            &log.message,
            crate::sentry_reporting::level_from_name(&log.level),
            None,
            &[],
            context.as_ref().map(|c| ("esp-data", c)),
            firmware.as_deref(),
        );
    }
}

/// The `if notify is not None` block.
fn handle_notify(app: &Arc<App>, notify: esp::MachineNotify) {
    let response_options: Vec<&str> = if notify.notification_type == "acaia_msg" {
        vec![]
    } else {
        vec![responses::OK]
    };
    let notification = {
        let mut state = app.machine.state.lock();
        let acknowledged = match &state.esp_notification {
            None => false,
            Some(current) => app
                .notifications
                .is_acknowledged(&current.id)
                .unwrap_or(state.esp_notification_shown),
        };
        let notification = match state.esp_notification.take() {
            Some(mut current) if !acknowledged => {
                current.message = notify.message.clone();
                current.responses = response_options.iter().map(|r| r.to_string()).collect();
                current
            }
            _ => Notification::with_responses(notify.message.clone(), &response_options),
        };
        state.esp_notification = Some(notification.clone());
        state.esp_notification_shown = true;
        notification
    };
    tracing::info!("New Notification from ESP: {}", notification.message);
    app.notifications.add(notification);
}

/// Python's `repr(bytes)` for the "decoding fails" log line.
fn python_bytes_repr(bytes: &[u8]) -> String {
    let mut out = String::from("b'");
    for &b in bytes {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\'' => out.push_str("\\'"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push('\'');
    out
}
