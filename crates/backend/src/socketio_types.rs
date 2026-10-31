//! The TypeScript types of the Socket.IO payloads, `bindings/socketio.d.ts`.
//!
//! Each payload is built by hand as a `serde_json::Value` (Python's key order
//! and number types); the [`tstype::Ts`] next to each builder says what
//! TypeScript sees. The tests hold the builders' values for representative
//! inputs against those types, demand that every part of each type is
//! exercised, and compare the generated file with the committed one. In test
//! builds every emit is checked against its type as well (`Sio::record`).

use tstype::{Conflict, Ts};

use crate::bug_report::progress::ReportProgress;
use crate::contract::EMITS;
use crate::notifications::Notification;
use crate::os_update::OsUpdateStatus;
use crate::{config, events, profiles};

/// Where the generated declarations are committed, from the repository root.
pub const BINDINGS_PATH: &str = "bindings/socketio.d.ts";

const HEADER: &str = "\
// The payloads of the Socket.IO events meticulous-backend emits, generated from
// the types next to the Rust code that builds each one (docs/rust/ARCHITECTURE.md).
// Do not edit: `UPDATE_CONTRACTS=1 cargo test -p meticulous-backend socketio_types`
// rewrites it; `meticulous-backend contract --typescript` prints it.

";

/// The type of `event`'s payload.
pub fn payload_type(event: &str) -> Option<Ts> {
    Some(match event {
        "OSUpdate" => OsUpdateStatus::sio_type(),
        "button" => esp::ButtonEventData::sio_type(),
        "heater_status" => esp::HeaterTimeoutInfo::sio_heater_status_type(),
        "notification" => Notification::sio_type(),
        "profile" => profiles::profile_event_type(),
        "profileHover" => profiles::Hover::sio_type(),
        "report_progress" => ReportProgress::payload_type(),
        "sensors" => esp::SensorData::sio_sensors_type(),
        "settings" => config::settings_event_type(),
        "status" => events::status_type(),
        "upload_report" => history::bug_reports::BugReportRow::dispatch_payload_type(),
        _ => return None,
    })
}

/// Every event in [`EMITS`] that has a type, with it.
pub fn payload_types() -> Vec<(&'static str, Ts)> {
    EMITS
        .iter()
        .filter_map(|&event| payload_type(event).map(|ty| (event, ty)))
        .collect()
}

/// `bindings/socketio.d.ts`: one declaration per named type, the payloads by
/// event name, and the listener map socket.io-client takes.
pub fn typescript() -> Result<String, Conflict> {
    let types = payload_types();
    let mut out = String::from(HEADER);
    out.push_str(&tstype::declarations(types.iter().map(|(_, ty)| ty))?);
    out.push_str("\n/** Every event the backend emits, with its payload. */\n");
    out.push_str("export interface SocketIoPayloads {\n");
    for (event, ty) in &types {
        out.push_str(&format!(
            "  {}: {};\n",
            tstype::key(event),
            tstype::inline(ty)
        ));
    }
    out.push_str("}\n\n");
    out.push_str(
        "/** The listeners, for socket.io-client's `Socket<ServerToClientEvents, ...>`. */\n",
    );
    out.push_str("export type ServerToClientEvents = {\n");
    out.push_str("  [E in keyof SocketIoPayloads]: (data: SocketIoPayloads[E]) => void;\n");
    out.push_str("};\n");
    Ok(out)
}

/// Panic unless `data` is of `event`'s payload type: every emit of every
/// test goes through here.
#[cfg(test)]
pub fn assert_conforms(event: &str, data: &serde_json::Value) {
    static TYPES: std::sync::LazyLock<Vec<(&'static str, Ts)>> =
        std::sync::LazyLock::new(payload_types);
    let Some((_, ty)) = TYPES.iter().find(|(e, _)| *e == event) else {
        panic!("{event} has no payload type in socketio_types::payload_type");
    };
    if let Err(error) = tstype::check(event, data, ty) {
        panic!(
            "the {event} payload does not match {}: {error}\n{data}\nfix the builder, or the type next to it",
            tstype::inline(ty)
        );
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::sync::Arc;

    use serde_json::{Value, json};

    use super::*;
    use crate::app::App;
    use crate::bug_report::progress::{Failure, Origin, Stage, StageDetail, StageUpdate};
    use crate::bug_report::{self as reports, test_support};
    use crate::config::keys;
    use crate::notifications::responses;
    use crate::os_update::OsStatus;
    use crate::profiles::{POUR_OVER_BREW_TYPE, ProfileEvent};

    const PROFILE_ID: &str = "05051ed3-9996-43e8-9da6-963f2b31d481";

    fn repository() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// The bindings are committed as generated; `UPDATE_CONTRACTS=1`
    /// rewrites them, like the contract snapshots.
    #[test]
    fn bindings_match_the_committed_file() {
        let path = repository().join(BINDINGS_PATH);
        let current = typescript().expect("no two payload types share a name");
        if std::env::var("UPDATE_CONTRACTS").as_deref() == Ok("1") {
            std::fs::create_dir_all(path.parent().expect("bindings directory")).unwrap();
            std::fs::write(&path, &current).unwrap();
            return;
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            current == committed,
            "{BINDINGS_PATH} is stale; if intended run UPDATE_CONTRACTS=1 cargo test -p meticulous-backend socketio_types and commit it\n--- committed\n{committed}\n+++ generated\n{current}"
        );
    }

    #[test]
    fn every_emitted_event_has_a_named_payload_type() {
        for event in EMITS {
            let ty = payload_type(event).unwrap_or_else(|| panic!("{event} has no type"));
            assert!(ty.name().is_some(), "{event}'s type has no name");
        }
    }

    /// The ESP32 lines `crates/esp/tools/gen_goldens.py` ran through the
    /// Python: real frames and the malformed ones it generates.
    fn golden_lines() -> Vec<String> {
        let path = repository().join("crates/esp/testdata/goldens/lines.json");
        let cases: Vec<Value> =
            serde_json::from_str(&std::fs::read_to_string(path).expect("goldens")).unwrap();
        cases
            .iter()
            .filter_map(|case| case["line"].as_str().map(str::to_string))
            .collect()
    }

    /// `status`, `sensors`, `button` and `heater_status` for the golden
    /// lines, as the reader and `live()` build them.
    fn machine_payloads(app: &App, out: &mut Vec<(String, Value)>) {
        let last_profile = json!({
            "load_time": 1.7e9,
            "profile": {"name": "Italian", "id": PROFILE_ID},
        });
        out.push(("status".into(), events::status(app)));
        let extra = [
            // The emulator's frames, which stand for the loaded profile.
            format!(
                "Data,1.5,0.8,36.2,S,92.1,brewing,{}",
                crate::machine::emulator::PROFILE_PLACEHOLDER
            ),
            // A motor thermistor out of range.
            "Sensors,90.1,91.2,92,93,94,95,85,40,50,10.5,100,5,2.5,1,3,9,0.1,0.2,0.3,0.4,true,inf,18.5"
                .to_string(),
            // The Kalman readings of disturbance resilience firmware, not a
            // number ("NaN") and infinite (null) each way round.
            "Sensors,90.1,91.2,92,93,94,95,85,40,50,10.5,100,5,2.5,1,3,9,0.1,0.2,0.3,0.4,true,40,18.5,nan,inf,"
                .to_string(),
            "Sensors,90.1,91.2,92,93,94,95,85,40,50,10.5,100,5,2.5,1,3,9,0.1,0.2,0.3,0.4,true,40,18.5,inf,nan,"
                .to_string(),
        ];
        for (i, line) in golden_lines().iter().chain(&extra).enumerate() {
            match esp::classify_line(line) {
                esp::EspLine::Data(Some(shot)) => {
                    let brewing = i % 2 == 0;
                    app.machine.state.lock().data_sensors = shot.clone_with_time_and_state(
                        if brewing { 1234 } else { 0 },
                        brewing,
                        900,
                    );
                    let profile = if i % 3 == 0 {
                        Value::Null
                    } else {
                        last_profile.clone()
                    };
                    app.config.set(keys::PROFILES, keys::PROFILE_LAST, profile);
                    out.push(("status".into(), events::status(app)));
                }
                esp::EspLine::Sensors(Some(sensors)) => {
                    out.push(("sensors".into(), sensors.to_sio_sensors()));
                }
                esp::EspLine::Event(Some(button)) => out.push(("button".into(), button.to_sio())),
                esp::EspLine::HeaterTimeoutInfo(Ok(heater)) => {
                    out.push(("heater_status".into(), heater.to_sio_heater_status()));
                }
                _ => {}
            }
        }
        for event in esp::ButtonEvent::ALL {
            let button = esp::ButtonEventData {
                event,
                time_since_last_event: 10000,
            };
            out.push(("button".into(), button.to_sio()));
        }
    }

    /// What the services emit, as `Sio` records it.
    fn service_payloads(app: &App, out: &mut Vec<(String, Value)>) {
        app.notifications.add(Notification::new("Descaling done"));
        app.notifications.add(
            Notification::with_responses(
                "Update the firmware?",
                &[responses::UPDATE, responses::SKIP],
            )
            .image("data:image/png;base64,iVBORw0KGgo="),
        );

        for change in ProfileEvent::ALL {
            app.profiles
                .emit_event(change, Some(PROFILE_ID), None, None);
        }
        app.profiles
            .emit_event(ProfileEvent::Update, Some(PROFILE_ID), Some("c1"), None);
        app.profiles.emit_event(
            ProfileEvent::Create,
            Some(PROFILE_ID),
            Some("c2"),
            Some(POUR_OVER_BREW_TYPE),
        );
        app.profiles
            .emit_event(ProfileEvent::Reload, None, None, None);

        app.profiles.handle_hover(
            &json!({"id": PROFILE_ID, "type": "focus", "from": "dial"}),
            "sid-1",
        );
        app.profiles
            .handle_hover(&json!({"id": 5, "type": null}), "sid-1");
        app.profiles.emit_hover_to("sid-2");

        app.config.save();

        for status in OsStatus::ALL {
            app.os_update.send_status(status, 42.5, None);
        }
        app.os_update.set_last_extra_info(Some("bundle rejected"));
        app.os_update
            .send_status(OsStatus::Failed, 100.0, Some("bundle rejected"));

        out.extend(app.sio.take_recorded());
    }

    /// `upload_report` and `report_progress` from two dispatches and a Dial
    /// report that fails, and `report_progress` at every stage.
    async fn report_payloads(out: &mut Vec<(String, Value)>) {
        let (_dir, app) = test_support::emulated_app();
        app.reports.set_now(Some(1_700_000_000));
        let dispatch = |body: Value| {
            let app: Arc<App> = app.clone();
            async move {
                let body = pyfmt::dumps_default(&body);
                let reply = crate::testing::call(
                    &app,
                    http::Method::POST,
                    "/api/v1/reports/dispatch",
                    body,
                )
                .await;
                assert_eq!(reply.status_code(), 202);
            }
        };
        dispatch(json!({"localID": reports::new_local_id(), "ticket": 4242})).await;
        app.config
            .set(keys::SYSTEM, keys::MACHINE_SERIAL_NUMBER, json!("MT-0001"));
        dispatch(json!({
            "localID": reports::new_local_id(),
            "ticket": 4243,
            "issueTime": 1_699_999_000,
            "description": "Grinder jammed",
            "name": "Ada",
            "email": "ada@example.com",
        }))
        .await;

        let local_id = reports::new_local_id();
        let started = StageUpdate {
            origin: Some(Origin::Dial),
            ..StageUpdate::default()
        };
        reports::report_progress(&app, &local_id, Stage::Preparing, started);
        let copying = StageUpdate {
            detail: Some(StageDetail {
                current: 1,
                total: 3,
            }),
            ..StageUpdate::default()
        };
        reports::report_progress(&app, &local_id, Stage::CollectingDebugShots, copying);
        let failed = StageUpdate {
            failure: Some(Failure {
                code: "INTERNAL".into(),
                message: "Upload failed".into(),
            }),
            ..StageUpdate::default()
        };
        reports::report_progress(&app, &local_id, Stage::Failed, failed);
        out.extend(
            app.reports
                .hooks
                .emitted
                .lock()
                .iter()
                .map(|(event, value)| (event.to_string(), value.clone())),
        );

        for origin in Origin::ALL {
            for stage in Stage::ALL {
                let entry =
                    ReportProgress::new("local", origin, json!(7), stage, json!(1), json!(2));
                out.push(("report_progress".into(), entry.payload()));
            }
        }
    }

    /// Each payload type against what its builder sends for representative
    /// inputs: every value of its type, and every union member, optional key
    /// (present and absent) and index signature of the type sent at least
    /// once, so the type promises no more and no less than the wire carries.
    #[tokio::test]
    async fn payloads_match_their_types_and_exercise_all_of_them() {
        let (_dir, app) = crate::testing::app();
        app.sio.take_recorded();
        let mut samples = Vec::new();
        machine_payloads(&app, &mut samples);
        service_payloads(&app, &mut samples);
        report_payloads(&mut samples).await;

        let mut problems = BTreeSet::new();
        for (event, _) in &samples {
            if !EMITS.contains(&event.as_str()) {
                problems.insert(format!("{event} is not in EMITS"));
            }
        }
        for event in EMITS {
            let ty = payload_type(event).expect("every event has a type");
            let mut coverage = tstype::Coverage::new(event, &ty);
            for (_, value) in samples.iter().filter(|(e, _)| e == event) {
                if let Err(error) = coverage.check(value) {
                    problems.insert(format!("{error} in {value}"));
                }
            }
            if coverage.values() == 0 {
                problems.insert(format!("{event}: no sample"));
            }
            for fact in coverage.missing() {
                problems.insert(format!("no sample exercises {fact}"));
            }
        }
        assert!(
            problems.is_empty(),
            "payloads and their TypeScript types disagree; fix the builder, the type next to it, or the samples here:\n{}",
            problems.into_iter().collect::<Vec<_>>().join("\n")
        );
    }
}
