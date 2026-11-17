//! tests/test_api_machine.py: `GET /machine`, the firmware upload
//! (`POST /update/firmware`) and the time zone picker (`GET /timezones/`).
//! The other classes of the file are ported next to their routes
//! (`api/machine.rs`, `ota.rs`).

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::app::App;
use crate::config::keys;
use crate::http::Reply;
use crate::testing::{app, call, call_with_headers, json as body};

// -- TestMachineInfoRoute ----------------------------------------------------

/// The hostname comes from NetworkManager: a station on `HomeNet` whose
/// host is `meticulousAromaticbean-1234`.
fn machine_on_home_network(app: &App) {
    let network = crate::wifi::fake::FakeNetwork::new();
    network.add_connection("HomeNet", "wifi");
    network.connect_home_station("HomeNet");
    app.wifi.set_host(network);
}

/// `Machine.esp_info = ESPInfo(...)`.
fn esp_reports(app: &App, info: Option<esp::EspInfo>) {
    app.machine.state.lock().esp_info = info;
}

fn write_image_metadata(root: &std::path::Path) {
    let opt = root.join("opt");
    std::fs::create_dir_all(&opt).expect("opt");
    std::fs::write(
        opt.join("ROOTFS_BUILD_DATE"),
        "Tue, 06 Oct 2026 12:34:56 +0000\n",
    )
    .expect("build date");
    std::fs::write(opt.join("image-build-channel"), "beta\n").expect("channel");
    std::fs::write(opt.join("image-build-version"), "2026M1446-beta\n").expect("version");
    std::fs::write(
        opt.join("summary.txt"),
        concat!(
            "## meticulous-backend ##\n",
            "Repository: meticulous-backend\n",
            "URL: https://github.com/MeticulousHome/meticulous-backend\n",
            "Branch: beta\n",
            "Commit: abc1234\n",
            "Last commit details:\n",
            "abc1234 Fix wifi repair (2026-10-01)\n",
            "Modified files:\n",
            "\n",
            "## meticulous-dial ##\n",
            "Repository: meticulous-dial\n",
            "Branch: main\n",
            "Commit: def5678\n",
        ),
    )
    .expect("summary");
}

async fn get_machine(app: &Arc<App>) -> Reply {
    call(app, http::Method::GET, "/api/v1/machine", "").await
}

/// tests/test_api_machine.py::TestMachineInfoRoute::test_reports_every_machine_field_from_esp_config_and_image_metadata
#[tokio::test]
async fn reports_every_machine_field_from_esp_config_and_image_metadata() {
    let (dir, app) = app();
    machine_on_home_network(&app);
    let history = json!([{"version": "2026M1400-beta", "date": "2026-09-01"}]);
    let system = [
        (keys::DEVICE_IDENTIFIER, json!(["aromatic", "bean"])),
        (keys::MACHINE_SERIAL_NUMBER, json!("1234")),
        (keys::MACHINE_COLOR, json!("black")),
        (keys::MACHINE_BATCH_NUMBER, json!("B-07")),
        (keys::MACHINE_BUILD_DATE, json!("2026-01-15")),
        (keys::LAST_SYSTEM_VERSIONS, history.clone()),
    ];
    for (key, value) in system {
        app.config.set(keys::SYSTEM, key, value);
    }
    esp_reports(
        &app,
        Some(esp::EspInfo {
            firmware_v: "1.2.3".into(),
            esp_pinout: 1,
            main_voltage: 230.5,
            tare_behavior: Some("auto".into()),
            ..esp::EspInfo::default()
        }),
    );
    app.machine.toggle_manufacturing_mode(&app, true);
    write_image_metadata(dir.path());
    // UpdateManager.init finds an image it has not seen (upgrade_first_boot)
    // and records it in the history, which the Python test sets by hand.
    app.ota.init();
    app.config
        .set(keys::SYSTEM, keys::LAST_SYSTEM_VERSIONS, history);
    app.pour_over.init();

    let reply = get_machine(&app).await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(reply.header("Content-Type"), Some("application/json"));
    assert_eq!(
        body(&reply),
        json!({
            "name": "MeticulousAromaticbean",
            "hostname": "meticulousAromaticbean-1234",
            "firmware": "1.2.3",
            "mainVoltage": 230.5,
            "tare_behavior_supported": true,
            "pour_over_profiles_supported": true,
            "pour_over_profile_schema_version": 1,
            "serial": "1234",
            "color": "black",
            "batch_number": "B-07",
            "build_date": "2026-01-15",
            "software_version": "2026-10-06 12:34:56",
            "image_build_channel": "beta",
            "image_version": "2026M1446-beta",
            "repository_info": {
                "meticulous-backend": {
                    "branch": "beta",
                    "commit": "abc1234 Fix wifi repair (2026-10-01)",
                },
                "meticulous-dial": {"branch": "main", "commit": null},
            },
            "manufacturing": true,
            "upgrade_first_boot": true,
            "version_history": [{"version": "2026M1400-beta", "date": "2026-09-01"}],
        })
    );
}

/// tests/test_api_machine.py::TestMachineInfoRoute::test_fresh_machine_without_esp_or_image_metadata_reports_empty_defaults
#[tokio::test]
async fn fresh_machine_without_esp_or_image_metadata_reports_empty_defaults() {
    let (_dir, app) = app();
    machine_on_home_network(&app);
    app.config
        .set(keys::SYSTEM, keys::DEVICE_IDENTIFIER, json!([]));
    for key in [
        keys::MACHINE_SERIAL_NUMBER,
        keys::MACHINE_COLOR,
        keys::MACHINE_BATCH_NUMBER,
        keys::MACHINE_BUILD_DATE,
        keys::LAST_SYSTEM_VERSIONS,
    ] {
        app.config.set(keys::SYSTEM, key, Value::Null);
    }
    esp_reports(&app, None);

    let reply = get_machine(&app).await;

    assert_eq!(reply.status_code(), 200);
    // firmware and mainVoltage are omitted, not null, until the ESP reports in.
    assert_eq!(
        body(&reply),
        json!({
            "name": "MeticulousEspresso",
            "hostname": "meticulousAromaticbean-1234",
            "tare_behavior_supported": false,
            "pour_over_profiles_supported": false,
            "pour_over_profile_schema_version": null,
            "serial": null,
            "color": "",
            "batch_number": "",
            "build_date": "",
            "software_version": null,
            "image_build_channel": null,
            "image_version": null,
            "repository_info": {},
            "manufacturing": false,
            "upgrade_first_boot": false,
            "version_history": [],
        })
    );
}

/// tests/test_api_machine.py::TestMachineInfoRoute::test_firmware_without_tare_behavior_reports_tare_behavior_unsupported
#[tokio::test]
async fn firmware_without_tare_behavior_reports_tare_behavior_unsupported() {
    let (_dir, app) = app();
    machine_on_home_network(&app);
    esp_reports(
        &app,
        Some(esp::EspInfo {
            firmware_v: "1.0.0".into(),
            esp_pinout: 0,
            main_voltage: 120.0,
            ..esp::EspInfo::default()
        }),
    );

    let machine = body(&get_machine(&app).await);

    assert_eq!(machine["firmware"], json!("1.0.0"));
    assert_eq!(machine["mainVoltage"], json!(120.0));
    assert_eq!(machine["tare_behavior_supported"], json!(false));
}

/// Deliberate fix (machine_info.rs): when the network configuration cannot
/// be read (here NetworkManager reports an address that does not parse) the
/// Python answers 500; the hostname is null instead.
#[tokio::test]
async fn unreadable_network_configuration_reports_a_null_hostname() {
    let (_dir, app) = app();
    let network = crate::wifi::fake::FakeNetwork::new();
    network.add_connection("HomeNet", "wifi");
    network.connect_station("HomeNet", Some("not an address"), None, None);
    app.wifi.set_host(network);

    let reply = get_machine(&app).await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(body(&reply)["hostname"], Value::Null);
    assert_eq!(body(&reply)["name"], json!("MeticulousEspresso"));
}

// -- TestUpdateFirmwareRoute -------------------------------------------------

const BOUNDARY: &str = "gate-boundary-7f3a";
const NOT_REFRESHED: &str = "firmware list not refreshed";

fn multipart_body(field: &str, filename: &str, content: &[u8]) -> Vec<u8> {
    let mut body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

fn zip_bytes(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, content) in entries {
        archive.start_file(*name, options).expect("entry");
        archive.write_all(content).expect("content");
    }
    archive.finish().expect("zip").into_inner()
}

/// The firmware folder, and a stand-in for the flasher's two steps: the
/// list of available firmware is replaced by what `refreshAvailableFirmware`
/// finds on disk, and `startUpdate` without an ESP32 to flash ends with its
/// failure notification.
fn firmware_app() -> (tempfile::TempDir, Arc<App>, std::path::PathBuf) {
    let (dir, app) = app();
    app.machine.state.lock().firmware_available_string = Some(NOT_REFRESHED.into());
    let root = app.env.paths.firmware.clone();
    (dir, app, root)
}

fn firmware_refreshed(app: &App) -> bool {
    app.machine
        .state
        .lock()
        .firmware_available_string
        .as_deref()
        != Some(NOT_REFRESHED)
}

fn update_attempts(app: &App) -> usize {
    app.notifications
        .all()
        .iter()
        .filter(|n| n.message.starts_with("Realtime core upgrade failed"))
        .count()
}

async fn wait_for_update(app: &App) -> bool {
    for _ in 0..500 {
        if update_attempts(app) > 0 {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

async fn upload(app: &Arc<App>, query: &str, filename: &str, content: &[u8]) -> Reply {
    let content_type = format!("multipart/form-data; boundary={BOUNDARY}");
    call_with_headers(
        app,
        http::Method::POST,
        &format!("/api/v1/update/firmware{query}"),
        multipart_body("file", filename, content),
        &[("Content-Type", &content_type)],
    )
    .await
}

fn assert_no_flash_started(app: &App) {
    assert!(!firmware_refreshed(app));
    assert_eq!(update_attempts(app), 0);
}

/// tests/test_api_machine.py::TestUpdateFirmwareRoute::test_missing_chip_parameter_is_rejected
#[tokio::test]
async fn missing_chip_parameter_is_rejected() {
    let (_dir, app, _root) = firmware_app();

    let reply = upload(&app, "", "firmware.bin", b"\xe9firmware").await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(reply.body(), b"Missing 'chip' parameter");
    assert_no_flash_started(&app);
}

/// tests/test_api_machine.py::TestUpdateFirmwareRoute::test_unknown_chip_is_rejected_with_the_allowed_names
#[tokio::test]
async fn unknown_chip_is_rejected_with_the_allowed_names() {
    let (_dir, app, _root) = firmware_app();

    let reply = upload(&app, "?chip=esp8266", "firmware.bin", b"\xe9firmware").await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        reply.body(),
        b"Invalid 'chip' parameter. Allowed (case-insensitive): ['ESP32S1', 'ESP32S3']"
    );
    assert_no_flash_started(&app);
}

/// tests/test_api_machine.py::TestUpdateFirmwareRoute::test_request_without_file_is_rejected
#[tokio::test]
async fn request_without_file_is_rejected() {
    let (_dir, app, _root) = firmware_app();

    let reply = call(
        &app,
        http::Method::POST,
        "/api/v1/update/firmware?chip=esp32s3",
        "",
    )
    .await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(reply.body(), b"No file uploaded.");
    assert_no_flash_started(&app);
}

/// tests/test_api_machine.py::TestUpdateFirmwareRoute::test_file_that_is_neither_zip_nor_known_image_is_rejected
#[tokio::test]
async fn file_that_is_neither_zip_nor_known_image_is_rejected() {
    let (_dir, app, root) = firmware_app();

    let reply = upload(&app, "?chip=esp32s3", "firmware.hex", b":1000").await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        reply.body(),
        b"Invalid file format. Only ZIP files and certain images are accepted."
    );
    assert!(!root.exists());
    assert_no_flash_started(&app);
}

/// tests/test_api_machine.py::TestUpdateFirmwareRoute::test_single_image_is_stored_in_the_chip_folder_and_flashing_starts
#[tokio::test]
async fn single_image_is_stored_in_the_chip_folder_and_flashing_starts() {
    let (_dir, app, root) = firmware_app();

    let reply = upload(&app, "?chip=ESP32-S3", "firmware.bin", b"\xe9firmware").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(reply.body(), b"success");
    assert_eq!(
        std::fs::read(root.join("esp32-s3/firmware.bin")).expect("stored"),
        b"\xe9firmware"
    );
    assert!(firmware_refreshed(&app));
    assert!(wait_for_update(&app).await);
    assert_eq!(update_attempts(&app), 1);
}

/// tests/test_api_machine.py::TestUpdateFirmwareRoute::test_zip_is_unpacked_into_the_chip_folder_and_flashing_starts
#[tokio::test]
async fn zip_is_unpacked_into_the_chip_folder_and_flashing_starts() {
    let (_dir, app, root) = firmware_app();
    let archive = zip_bytes(&[
        ("firmware.bin", b"\xe9fw".as_slice()),
        ("partitions.bin", b"parts".as_slice()),
    ]);

    let reply = upload(&app, "?chip=esp32", "esp32-firmware.zip", &archive).await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(reply.body(), b"success");
    assert_eq!(
        std::fs::read(root.join("esp32/firmware.bin")).expect("firmware"),
        b"\xe9fw"
    );
    assert_eq!(
        std::fs::read(root.join("esp32/partitions.bin")).expect("partitions"),
        b"parts"
    );
    assert!(firmware_refreshed(&app));
    assert!(wait_for_update(&app).await);
}

/// tests/test_api_machine.py::TestUpdateFirmwareRoute::test_corrupted_zip_reports_failure_and_never_flashes
#[tokio::test]
async fn corrupted_zip_reports_failure_and_never_flashes() {
    let (_dir, app, _root) = firmware_app();

    let reply = upload(
        &app,
        "?chip=esp32s3",
        "firmware.zip",
        b"PK\x03\x04 not a zip",
    )
    .await;

    assert_eq!(reply.status_code(), 400);
    assert_eq!(
        reply.body(),
        b"The uploaded file is not a valid ZIP archive.failure during upload"
    );
    assert_no_flash_started(&app);
}

// -- TestTimezonesRoute -------------------------------------------------------

/// The picker's table; an existing file short-circuits the timedatectl-based
/// generator.
fn timezones_app() -> (tempfile::TempDir, Arc<App>) {
    let (dir, app) = app();
    let path = &app.env.paths.timezone_json;
    std::fs::create_dir_all(path.parent().expect("parent")).expect("zoneinfo");
    let table = json!({
        "Mexico": {"Cancun": "America/Cancun", "Mexico City": "America/Mexico_City"},
        "Germany": {"Berlin": "Europe/Berlin", "Busingen": "Europe/Busingen"},
        "Netherlands": {"Amsterdam": "Europe/Amsterdam"},
    });
    std::fs::write(path, pyfmt::dumps_default(&table)).expect("timezones");
    (dir, app)
}

async fn get(app: &Arc<App>, uri: &str) -> Reply {
    call(app, http::Method::GET, uri, "").await
}

/// tests/test_api_machine.py::TestTimezonesRoute::test_empty_region_lists_all_countries
#[tokio::test]
async fn empty_region_lists_all_countries() {
    let (_dir, app) = timezones_app();

    let reply = get(&app, "/api/v1/timezones/").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        body(&reply),
        json!({"countries": ["Mexico", "Germany", "Netherlands"]})
    );
}

/// tests/test_api_machine.py::TestTimezonesRoute::test_countries_are_filtered_by_lowercase_prefix
#[tokio::test]
async fn countries_are_filtered_by_lowercase_prefix() {
    let (_dir, app) = timezones_app();

    let reply = get(&app, "/api/v1/timezones/countries?filter=ne").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(body(&reply), json!({"countries": ["Netherlands"]}));
}

/// tests/test_api_machine.py::TestTimezonesRoute::test_country_filter_is_case_insensitive
/// (a strict xfail in the Python suite).
#[tokio::test]
async fn country_filter_is_case_insensitive() {
    let (_dir, app) = timezones_app();

    let reply = get(&app, "/api/v1/timezones/countries?filter=Mex").await;

    // PARITY: TimezoneUIHandler lowercases the country but not the filter,
    // so a capitalised filter matches nothing (the Python test expects
    // ["Mexico"]).
    assert_eq!(reply.status_code(), 200);
    assert_eq!(body(&reply), json!({"countries": []}));
}

/// tests/test_api_machine.py::TestTimezonesRoute::test_cities_of_a_country_are_listed_as_name_to_zone_pairs
#[tokio::test]
async fn cities_of_a_country_are_listed_as_name_to_zone_pairs() {
    let (_dir, app) = timezones_app();

    let reply = get(&app, "/api/v1/timezones/cities?filter=Mexico").await;

    assert_eq!(reply.status_code(), 200);
    assert_eq!(
        body(&reply),
        json!({"cities": [
            {"Cancun": "America/Cancun"},
            {"Mexico City": "America/Mexico_City"},
        ]})
    );
}

/// tests/test_api_machine.py::TestTimezonesRoute::test_cities_of_an_unknown_country_are_rejected
#[tokio::test]
async fn cities_of_an_unknown_country_are_rejected() {
    let (_dir, app) = timezones_app();

    let reply = get(&app, "/api/v1/timezones/cities?filter=Atlantis").await;

    assert_eq!(reply.status_code(), 403);
    assert_eq!(
        body(&reply),
        json!({"status": "error", "error": "invalid country requested"})
    );
}

/// tests/test_api_machine.py::TestTimezonesRoute::test_unknown_region_type_is_rejected
#[tokio::test]
async fn unknown_region_type_is_rejected() {
    let (_dir, app) = timezones_app();

    let reply = get(&app, "/api/v1/timezones/planets").await;

    assert_eq!(reply.status_code(), 403);
    assert_eq!(
        body(&reply),
        json!({"status": "error", "error": "invalid region type requested"})
    );
}
