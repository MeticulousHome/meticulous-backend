//! The firmware's UART contract (`contracts/uart/protocol.json` in the
//! private EspressoFirmware repository) against this crate's parser and
//! encoders. Point `FIRMWARE_UART_CONTRACT` at the file to run it; without
//! it the test passes and says it was skipped.
//!
//! Every message in `from_firmware` is rebuilt from its field list and must
//! dispatch to the case the Python dispatches it to (or be free text the
//! backend ignores), every `to_firmware` key and action the backend sends
//! must exist, and every NVS key the backend uses must be listed. Where the
//! Python reads a message differently from the contract (fields it ignores,
//! types it changes), the test prints a finding: parity with the Python wins
//! until the backend changes on purpose. Anything new in the contract that
//! the backend does not know fails the test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use esp::encode::{self, NvsKey};
use esp::line::{EspLine, classify_line};
use esp::sync::DeviceUuidResponse;
use esp::{PyNum, ShotData};
use serde_json::Value;

/// How the Python reads each message it knows: the dispatch token, the
/// number of fields it reads after it, and whether that number is exact
/// (more or fewer fields make the line something else).
struct Known {
    name: &'static str,
    token: &'static str,
    fields_read: usize,
    exact: bool,
    /// The last field takes the rest of the line (commas included).
    rest: bool,
}

const KNOWN: &[Known] = &[
    Known {
        name: "Data",
        token: "Data",
        fields_read: 13,
        exact: false,
        rest: false,
    },
    Known {
        name: "Sensors",
        token: "Sensors",
        fields_read: 23,
        exact: false,
        rest: false,
    },
    Known {
        name: "ESPInfo",
        token: "ESPInfo",
        fields_read: 12,
        exact: false,
        rest: false,
    },
    Known {
        name: "ESPBoot",
        token: "ESPBoot",
        fields_read: 2,
        exact: true,
        rest: false,
    },
    Known {
        name: "HeaterTimeoutInfo",
        token: "HeaterTimeoutInfo",
        fields_read: 4,
        exact: true,
        rest: false,
    },
    Known {
        name: "MileageRequest",
        token: "MileageRequest",
        fields_read: 0,
        exact: true,
        rest: false,
    },
    Known {
        name: "Event",
        token: "Event",
        fields_read: 2,
        exact: false,
        rest: false,
    },
    Known {
        name: "Notify",
        token: "Notify",
        fields_read: 2,
        exact: false,
        rest: true,
    },
    Known {
        name: "Log",
        token: "Log",
        fields_read: 3,
        exact: false,
        rest: true,
    },
    // nvs_response,<key>,<value> is matched with exactly two fields.
    Known {
        name: "nvs_response",
        token: "nvs_response",
        fields_read: 2,
        exact: true,
        rest: false,
    },
    Known {
        name: "device_uuid_response",
        token: "device_uuid_response",
        fields_read: 1,
        exact: true,
        rest: false,
    },
];

/// Messages the Python treats as free text (logged, not valid traffic).
const FREE_TEXT: &[&str] = &["hash_received", "manual_control", "calibration", "error"];

/// The `to_firmware` keys and the backend's encoder for each.
const KEYS_SENT: &[&str] = &[
    "action",
    "hash",
    "json",
    "nvs_request",
    "device_uuid",
    "heater_timeout",
];

/// Every action the backend writes: Socket.IO `action` (`ALLOWED_ESP_ACTIONS`),
/// `action,info` and backend.py's `calibrate`.
fn actions_sent() -> Vec<&'static str> {
    let mut actions: Vec<&str> = encode::ALLOWED_ESP_ACTIONS.to_vec();
    actions.extend(["info", "calibration"]);
    actions.sort_unstable();
    actions.dedup();
    actions
}

fn sample_value(field: &Value) -> Option<String> {
    for key in ["values", "values_documented"] {
        if let Some(first) = field
            .get(key)
            .and_then(|v| v.as_array())
            .and_then(|v| v.first())
        {
            return Some(first.as_str()?.to_string());
        }
    }
    Some(
        match field["type"].as_str()? {
            "float" => "1.5",
            "float|nan" => "nan",
            "int" => "3",
            "string" => "x",
            "urlencoded" => "a%20b",
            "rest" => "r",
            "enum" => return None,
            _ => return None,
        }
        .to_string(),
    )
}

fn build_line(message: &Value, values: &[String]) -> String {
    let prefix = message["prefix"].as_str().unwrap();
    let mut line = format!("{prefix}{}", values.join(","));
    if message.get("trailing_comma").and_then(Value::as_bool) == Some(true) {
        line.push(',');
    }
    line.push('\n');
    line
}

/// Whether the dispatch put the line where the Python puts that message,
/// with a successful parse.
fn dispatched_as(name: &str, line: &EspLine) -> bool {
    match (name, line) {
        ("Data", EspLine::Data(Some(_)))
        | ("Sensors", EspLine::Sensors(Some(_)))
        | ("ESPInfo", EspLine::EspInfo(Some(_)))
        | ("ESPBoot", EspLine::EspBoot { .. })
        | ("HeaterTimeoutInfo", EspLine::HeaterTimeoutInfo(Ok(_)))
        | ("MileageRequest", EspLine::MileageRequest)
        | ("Event", EspLine::Event(Some(_)))
        | ("Notify", EspLine::Notify(_))
        | ("Log", EspLine::Log(Ok(_)))
        | ("nvs_response", EspLine::NvsResponse { .. })
        | ("device_uuid_response", EspLine::DeviceUuidResponse(_)) => true,
        (_, EspLine::Unknown) => FREE_TEXT.contains(&name),
        _ => false,
    }
}

struct Report {
    findings: Vec<String>,
    failures: Vec<String>,
}

fn check_from_firmware(contract: &Value, report: &mut Report) {
    let messages = contract["from_firmware"].as_array().unwrap();
    let names: Vec<&str> = messages
        .iter()
        .map(|m| m["name"].as_str().unwrap())
        .collect();
    for known in KNOWN {
        if !names.contains(&known.name) {
            report.findings.push(format!(
                "{}: parsed by the backend but not in the contract",
                known.name
            ));
        }
    }
    report.findings.push(
        "bare CCW/CW/push/pu_d/elng/ta_d/ta_l/strt lines and the color-coded Sensors line are \
         parsed by the backend but not in the contract"
            .to_string(),
    );

    for message in messages {
        let name = message["name"].as_str().unwrap();
        let prefix = message["prefix"].as_str().unwrap();
        let fields = message["fields"].as_array().unwrap();
        let Some(values) = fields.iter().map(sample_value).collect::<Option<Vec<_>>>() else {
            report
                .failures
                .push(format!("{name}: a field type the test cannot sample"));
            continue;
        };
        let line = build_line(message, &values);
        let classified = classify_line(&line);

        if FREE_TEXT.contains(&name) {
            if classified != EspLine::Unknown {
                report
                    .failures
                    .push(format!("{name}: expected free text, got {classified:?}"));
            } else {
                report.findings.push(format!(
                    "{name} ({prefix:?}): free text for the backend, logged at INFO and passed \
                     to observe_raw_line, not counted as valid traffic"
                ));
            }
            continue;
        }
        let Some(known) = KNOWN.iter().find(|k| k.name == name) else {
            report.failures.push(format!(
                "{name} ({prefix:?}): new message the backend does not parse (dispatched as {classified:?})"
            ));
            continue;
        };

        let exact = message.get("exact").and_then(Value::as_bool) == Some(true);
        let expected_prefix = if exact || known.fields_read == 0 {
            known.token.to_string()
        } else {
            format!("{},", known.token)
        };
        if prefix != expected_prefix {
            report.failures.push(format!(
                "{name}: prefix {prefix:?}, the backend matches {expected_prefix:?}"
            ));
        }
        if !dispatched_as(name, &classified) {
            report
                .failures
                .push(format!("{name}: {line:?} dispatched as {classified:?}"));
        }

        let contract_rest = fields
            .last()
            .is_some_and(|f| f["type"].as_str() == Some("rest"));
        if fields.len() != known.fields_read {
            let text = format!(
                "{name}: the contract lists {} fields, the backend reads {}{}",
                fields.len(),
                known.fields_read,
                if fields.len() > known.fields_read {
                    format!(
                        " (ignores {})",
                        fields[known.fields_read..]
                            .iter()
                            .map(|f| f["name"].as_str().unwrap_or("?"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                } else {
                    String::new()
                }
            );
            if known.exact {
                report.failures.push(text);
            } else {
                report.findings.push(text);
            }
        }
        if contract_rest && !known.rest {
            report.findings.push(format!(
                "{name}: the contract's last field takes the rest of the line, the backend \
                 matches exactly {} fields, so a value with a comma is handled as {}",
                known.fields_read,
                if name == "nvs_response" {
                    "an nvs_response error (logged as a warning)"
                } else {
                    "another message"
                }
            ));
        }

        // Every documented value of an enum field parses.
        for (index, field) in fields.iter().enumerate() {
            let Some(options) = field.get("values").and_then(Value::as_array) else {
                continue;
            };
            for option in options {
                let option = option.as_str().unwrap();
                let mut values = values.clone();
                values[index] = option.to_string();
                let classified = classify_line(&build_line(message, &values));
                if !dispatched_as(name, &classified) {
                    report.failures.push(format!(
                        "{name}.{}={option}: dispatched as {classified:?}",
                        field["name"]
                    ));
                }
                if name == "device_uuid_response"
                    && matches!(
                        DeviceUuidResponse::parse(option),
                        DeviceUuidResponse::Unrecognized(_)
                    )
                {
                    report
                        .failures
                        .push(format!("device_uuid_response {option}: not handled"));
                }
                if name == "Log" {
                    let level = option.to_lowercase();
                    if !["debug", "info", "warning", "error"].contains(&level.as_str()) {
                        report
                            .findings
                            .push(format!("Log level {option}: the backend does not log it"));
                    }
                }
            }
        }
        if let Some(documented) = fields
            .iter()
            .find_map(|f| f.get("values_documented").and_then(Value::as_array))
        {
            for option in documented.iter().filter_map(Value::as_str) {
                if option.contains('<') {
                    report.findings.push(format!(
                        "{name} value {option:?}: a template; the backend knows tare_pressed/\
                         _released, context_pressed/_released and encoder_button_pressed/\
                         _released, and drops any other <button> (the line is valid, no button event)"
                    ));
                    continue;
                }
                let mut values = values.clone();
                values[0] = option.to_string();
                let classified = classify_line(&build_line(message, &values));
                if !dispatched_as(name, &classified) {
                    report.failures.push(format!(
                        "{name} value {option}: dispatched as {classified:?}"
                    ));
                }
            }
        }

        if name == "ESPInfo" && fields.len() > 2 {
            let mut values = values.clone();
            values[2] = "120".to_string();
            if let EspLine::EspInfo(Some(info)) = classify_line(&build_line(message, &values)) {
                report.findings.push(format!(
                    "ESPInfo: field 2 ({}) is read as the pinout with int(), so {:?} gives {}; \
                     field 3 ({}, {}) is read with float() as main_voltage, so 120 is emitted \
                     as {} over Socket.IO",
                    fields[1]["name"],
                    values[1],
                    info.esp_pinout,
                    fields[2]["name"],
                    fields[2]["type"],
                    pyfmt::dumps_default(&info.to_sio()["main_voltage"])
                ));
            }
        }
    }
}

fn check_to_firmware(contract: &Value, report: &mut Report) {
    let link = &contract["link"];
    if link["backend_to_firmware"]["terminator"].as_str() != Some(encode::TERMINATOR) {
        report
            .failures
            .push("backend_to_firmware terminator differs from \\x03".to_string());
    }
    if link["firmware_to_backend"]["terminator"].as_str() != Some("\n") {
        report
            .failures
            .push("firmware_to_backend terminator is not \\n".to_string());
    }

    let entries = contract["to_firmware"].as_array().unwrap();
    let keys: Vec<&str> = entries.iter().map(|e| e["key"].as_str().unwrap()).collect();
    for key in KEYS_SENT {
        if !keys.contains(key) {
            report.failures.push(format!(
                "to_firmware: the backend sends {key}, the contract lacks it"
            ));
        }
    }
    for key in &keys {
        if !KEYS_SENT.contains(key) {
            report.failures.push(format!(
                "to_firmware {key}: new key without a backend encoder"
            ));
        }
    }

    // Each encoder's output has the documented key and field count.
    let upload = encode::send_json_with_hash(&serde_json::json!({"name": "x"}));
    let text = String::from_utf8(upload.bytes).unwrap();
    let messages: Vec<&str> = text.split(encode::TERMINATOR).collect();
    let samples: Vec<(&str, String, usize)> = vec![
        ("action", lossy(encode::action("info")), 2),
        ("action", lossy(encode::calibration(&PyNum::Float(18.5))), 4),
        ("hash", messages[0].to_string(), 2),
        ("nvs_request", lossy(encode::nvs_read(NvsKey::Mileage)), 3),
        (
            "nvs_request",
            lossy(encode::nvs_write(NvsKey::Color, "black")),
            4,
        ),
        ("device_uuid", lossy(encode::device_uuid_assign("x")), 3),
        (
            "heater_timeout",
            lossy(encode::heater_timeout(10).unwrap()),
            2,
        ),
    ];
    for (key, message, count) in samples {
        let body = message.trim_end_matches(encode::TERMINATOR);
        let parts: Vec<&str> = body.split(',').collect();
        if parts[0] != key || parts.len() != count {
            report
                .failures
                .push(format!("{key}: encoder wrote {body:?}"));
        }
    }
    if !messages[1].starts_with("json\n") {
        report
            .failures
            .push(format!("json: encoder wrote {:?}", messages[1]));
    }

    let action_entry = entries.iter().find(|e| e["key"] == "action").unwrap();
    let documented = action_entry["actions"].as_object().unwrap();
    for action in actions_sent() {
        match documented.get(action) {
            None => report.failures.push(format!(
                "action {action}: sent by the backend, not in the contract"
            )),
            Some(spec) => {
                if let Some(note) = spec.get("note").and_then(Value::as_str)
                    && note.contains("deprecated")
                {
                    report.findings.push(format!(
                        "action {action}: the backend sends it (ALLOWED_ESP_ACTIONS), the \
                             contract says {note:?}"
                    ));
                }
            }
        }
    }
    for action in documented.keys() {
        if !actions_sent().contains(&action.as_str()) {
            report.findings.push(format!(
                "action {action}: in the contract, never sent by the backend"
            ));
        }
    }
    let weight = ShotData::default().weight;
    report.findings.push(format!(
        "action calibration: the measured weight is str(Machine.data_sensors.weight), so it is \
         'NaN' while the load cell is inactive, which the firmware's float check may reject \
         ({:?} before the first Data line)",
        lossy(encode::calibration(&weight)).trim_end_matches(encode::TERMINATOR)
    ));
    report.findings.push(
        "nvs_request write: values are written as given; a value with a comma (serial, color, \
         tare behaviour) adds fields to a message the firmware splits on ','"
            .to_string(),
    );
}

fn check_nvs_keys(contract: &Value, report: &mut Report) {
    let listed: Vec<&str> = contract["nvs_keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["key"].as_str().unwrap())
        .collect();
    for key in NvsKey::ALL {
        if !listed.contains(&key.as_str()) {
            report.failures.push(format!(
                "nvs key {}: used by the backend, not in the contract",
                key.as_str()
            ));
        }
    }
    for key in &listed {
        if NvsKey::from_wire(key).is_none() {
            report.findings.push(format!(
                "nvs key {key}: in the contract, not used by the backend"
            ));
        }
    }
}

fn lossy(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).into_owned()
}

#[test]
fn firmware_uart_contract() {
    let Ok(path) = std::env::var("FIRMWARE_UART_CONTRACT") else {
        println!("FIRMWARE_UART_CONTRACT is not set: firmware UART contract check skipped");
        return;
    };
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let contract: Value = serde_json::from_str(&text).expect("the contract is JSON");
    let mut report = Report {
        findings: Vec::new(),
        failures: Vec::new(),
    };
    check_from_firmware(&contract, &mut report);
    check_to_firmware(&contract, &mut report);
    check_nvs_keys(&contract, &mut report);

    println!("UART contract version {} ({path})", contract["version"]);
    println!("findings (Python behaviour kept):");
    for finding in &report.findings {
        println!("  - {finding}");
    }
    assert!(
        report.failures.is_empty(),
        "contract not covered:\n  - {}",
        report.failures.join("\n  - ")
    );
}
