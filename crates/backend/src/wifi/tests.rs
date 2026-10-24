//! Unit tests of the Wi-Fi manager: tests/test_wifi_health_cache.py,
//! tests/test_wifi_list_cache.py, tests/test_wifi_repair.py, and the parts of
//! wifi.py those and the handler tests do not reach (init, auto-connect,
//! NetworkManager parsing, legacy migration, scanning).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::json;

use super::fake::{Call, FakeAnnouncer, FakeNetwork, HOSTNAME, MAC, WifiFixture, wpa2};
use super::types::{AP_CONNECTION, DeviceDetails, IpNet};
use super::*;
use crate::config::{WIFI_MODE_CLIENT, keys};
use crate::py::fake::RecordingRunner;
use crate::testing::app;

fn connected_config(name: &str) -> WifiSystemConfig {
    WifiSystemConfig {
        connected: true,
        connection_name: Some(name.into()),
        gateway: Some("192.168.1.1".parse().expect("ip")),
        routes: Vec::new(),
        ips: vec![IpNet::parse("192.168.1.42/24").expect("net")],
        dns: Vec::new(),
        mac: Some(String::new()),
        hostname: "meticulous".into(),
        domains: Vec::new(),
    }
}

fn setup() -> (tempfile::TempDir, Arc<crate::app::App>, WifiFixture) {
    let (dir, app) = app();
    let fixture = WifiFixture::install(&app);
    (dir, app, fixture)
}

fn ping_count(network: &FakeNetwork) -> usize {
    network.commands().iter().filter(|c| c[0] == "ping").count()
}

// ---- tests/test_wifi_health_cache.py ----

/// test_wifi_health_cache.py::test_shallow_entry_is_never_served_to_a_deep_caller
#[test]
fn shallow_entry_is_never_served_to_a_deep_caller() {
    let (_dir, app, f) = setup();
    app.wifi.invalidate_health_cache();
    let current = connected_config("Home");

    let shallow = app
        .wifi
        .get_health_status(Some(current.clone()), false, false)
        .expect("health");
    assert!(!shallow.verified);
    assert!(!shallow.internet_reachable); // unknown, not "unreachable"

    let deep = app
        .wifi
        .get_health_status(Some(current), false, true)
        .expect("health");
    // The deep caller ran the probes instead of reusing the shallow entry.
    assert_eq!(ping_count(&f.network), 1);
    assert_eq!(f.network.state.lock().dns_lookups.len(), 1);
    assert_eq!(f.network.state.lock().internet_calls, 1);
    assert!(deep.verified);
    assert!(deep.internet_reachable);
}

/// test_wifi_health_cache.py::test_shallow_caller_reuses_a_fresh_deep_entry
#[test]
fn shallow_caller_reuses_a_fresh_deep_entry() {
    let (_dir, app, f) = setup();
    app.wifi.invalidate_health_cache();
    let current = connected_config("Home");
    app.wifi
        .get_health_status(Some(current.clone()), false, true)
        .expect("deep");

    let shallow = app
        .wifi
        .get_health_status(Some(current), false, false)
        .expect("shallow");

    // Straight from the deep entry: no rebuild, no background refresh.
    assert!(shallow.verified);
    assert!(shallow.internet_reachable);
    assert!(f.network.threads().is_empty());
}

/// test_wifi_health_cache.py::test_shallow_path_does_not_block_on_the_probe_lock
#[test]
fn shallow_path_does_not_block_on_the_probe_lock() {
    let (_dir, app, _f) = setup();
    app.wifi.invalidate_health_cache();
    let current = connected_config("Home");

    // A deep probe in flight holds the lock.
    let guard = app.wifi.health_check_lock.lock();
    let (sender, receiver) = std::sync::mpsc::channel();
    let wifi = app.wifi.clone();
    std::thread::spawn(move || {
        let health = wifi.get_health_status(Some(current), false, false);
        let _ = sender.send(health);
    });
    let health = receiver
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("shallow path blocked on the deep probe lock")
        .expect("health");
    drop(guard);
    assert!(!health.verified);
}

/// test_wifi_health_cache.py::test_concurrent_shallow_and_deep_builds_never_tear_the_cache
#[test]
fn concurrent_shallow_and_deep_builds_never_tear_the_cache() {
    let (_dir, app, _f) = setup();
    app.wifi.invalidate_health_cache();
    let home = connected_config("Home");
    let guest = connected_config("Guest");
    let home_signature = app.wifi.health_cache_signature(Some(&home));
    let guest_signature = app.wifi.health_cache_signature(Some(&guest));
    assert_ne!(home_signature, guest_signature);

    let stop = Arc::new(AtomicBool::new(false));
    let worker = {
        let wifi = app.wifi.clone();
        let stop = stop.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let _ = wifi.build_health_status(Some(home.clone()), false);
            }
        })
    };
    for _ in 0..2000 {
        app.wifi
            .build_health_status(Some(guest.clone()), true)
            .expect("deep");
        let entry = app.wifi.health_cache_entry().expect("entry");
        let expected = if entry.health.verified {
            &guest_signature
        } else {
            &home_signature
        };
        assert_eq!(
            &entry.signature, expected,
            "cache entry tore across two builds"
        );
        assert!(entry.cached_at > 0.0);
    }
    stop.store(true, Ordering::Relaxed);
    let _ = worker.join();
}

/// test_wifi_health_cache.py::test_cache_entry_is_written_as_one_tuple
#[test]
fn cache_entry_is_written_as_one_tuple() {
    let (_dir, app, _f) = setup();
    app.wifi.invalidate_health_cache();
    let current = connected_config("Home");

    let health = app
        .wifi
        .get_health_status(Some(current.clone()), false, true)
        .expect("health");

    let entry = app.wifi.health_cache_entry().expect("entry");
    assert_eq!(entry.health, health);
    assert_eq!(
        entry.signature,
        app.wifi.health_cache_signature(Some(&current))
    );
    assert!(entry.cached_at > 0.0);
}

/// The cache expires after ten seconds and follows the network.
#[test]
fn cache_expires_and_follows_the_network() {
    let (_dir, app, f) = setup();
    let current = connected_config("Home");
    app.wifi
        .get_health_status(Some(current.clone()), false, true)
        .expect("deep");
    app.wifi
        .get_health_status(Some(current.clone()), false, true)
        .expect("cached");
    assert_eq!(ping_count(&f.network), 1);
    app.wifi
        .get_health_status(Some(connected_config("Other")), false, true)
        .expect("other network");
    assert_eq!(ping_count(&f.network), 2);
    f.network.sleep(10.0);
    app.wifi
        .get_health_status(Some(connected_config("Other")), false, true)
        .expect("expired");
    assert_eq!(ping_count(&f.network), 3);
}

// ---- tests/test_wifi_list_cache.py ----

/// test_wifi_list_cache.py::test_cold_start_blocks_on_scan_and_returns_real_list
#[test]
fn cold_start_blocks_on_scan_and_returns_real_list() {
    let (_dir, app, f) = setup();
    let scanned = vec![wpa2("Home", 70), wpa2("Cafe", 60)];
    f.network.state.lock().scan = scanned.clone();

    let result = app.wifi.get_available_networks(true, true);

    assert_eq!(result, scanned);
    assert_eq!(f.network.device_wifi_calls.load(Ordering::SeqCst), 1);
    assert!(f.network.threads().is_empty());
}

/// test_wifi_list_cache.py::test_populated_cache_never_blocks
#[test]
fn populated_cache_never_blocks() {
    let (_dir, app, f) = setup();
    app.wifi.set_scan_cache(vec![wpa2("Home", 70)]);

    let result = app.wifi.get_available_networks(true, true);

    assert_eq!(result, vec![wpa2("Home", 70)]);
    assert_eq!(f.network.device_wifi_calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.network.threads(), vec!["WifiScanRefresh"]);
}

/// test_wifi_list_cache.py::test_block_if_empty_defaults_off_and_returns_immediately
#[test]
fn block_if_empty_defaults_off_and_returns_immediately() {
    let (_dir, app, f) = setup();

    let result = app.wifi.get_available_networks(true, false);

    assert!(result.is_empty());
    assert_eq!(f.network.device_wifi_calls.load(Ordering::SeqCst), 0);
    assert_eq!(f.network.threads(), vec!["WifiScanRefresh"]);
}

/// test_wifi_list_cache.py::test_concurrent_cold_start_callers_share_one_scan
#[test]
fn concurrent_cold_start_callers_share_one_scan() {
    let (_dir, app, f) = setup();
    {
        let mut state = f.network.state.lock();
        state.scan = vec![wpa2("Home", 70)];
        state.scan_delay = std::time::Duration::from_millis(200);
    }
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let wifi = app.wifi.clone();
            std::thread::spawn(move || wifi.get_available_networks(true, true))
        })
        .collect();
    let results: Vec<_> = workers
        .into_iter()
        .map(|w| w.join().expect("join"))
        .collect();

    assert_eq!(f.network.device_wifi_calls.load(Ordering::SeqCst), 1);
    assert!(results.iter().all(|r| r == &vec![wpa2("Home", 70)]));
}

// ---- tests/test_wifi_repair.py ----

/// test_wifi_repair.py::test_repair_without_saved_client_connection_never_resets_hardware
#[test]
fn repair_without_saved_client_connection_never_resets_hardware() {
    let (_dir, app, f) = setup();

    let result = app
        .wifi
        .repair_wifi_connection("manual")
        .expect("no exception");

    assert!(!result);
    let state = app.wifi.state.lock().clone();
    assert_eq!(state.last_health_error, "no_saved_wifi_connection");
    assert_eq!(state.last_recovery_action, "health_check");
    assert_eq!(state.last_recovery_result, "not_recoverable");
    assert_eq!(state.auto_connect_suppressed_until, 0.0);
    assert!(f.network.calls().is_empty());
    assert!(f.network.commands().is_empty());
}

/// test_wifi_repair.py::test_disconnected_saved_connection_keeps_existing_repair_path
///
/// The Python test mocks `getHealthStatus`, which hides the `.connected`
/// bug. PARITY: the repair goes past the guard, assesses health with
/// `force=True` and raises.
#[test]
fn disconnected_saved_connection_keeps_existing_repair_path() {
    let (_dir, app, f) = setup();
    f.network.add_connection("Home", "wifi");

    let result = app.wifi.repair_wifi_connection("manual");

    assert_eq!(result, Err(health::no_connected_attribute()));
    let state = app.wifi.state.lock().clone();
    assert!(state.last_recovery_attempt > 0.0);
    assert_eq!(app.wifi.auto_connect_suppression_remaining(), 15);
    assert!(!app.wifi.repair_in_progress.load(Ordering::Relaxed));
}

/// The same repair once the health report is read correctly: a
/// disconnected radio goes through the reset steps.
#[test]
fn disconnected_saved_connection_runs_the_reset_steps_once_fixed() {
    let (_dir, app, f) = setup();
    app.wifi.set_health_has_no_connected(false);
    f.network.add_connection("Home", "wifi");

    let result = app
        .wifi
        .repair_wifi_connection("manual")
        .expect("no exception");

    assert!(!result);
    let commands = f.network.commands_except_type_lookups();
    assert!(commands.contains(&vec![
        "nmcli".to_string(),
        "radio".into(),
        "wifi".into(),
        "off".into()
    ]));
    assert!(commands.contains(&vec![
        "systemctl".to_string(),
        "restart".into(),
        "variscite-wifi.service".into()
    ]));
    let state = app.wifi.state.lock().clone();
    assert_eq!(state.last_recovery_result, "failed");
    assert_eq!(state.last_health_error, "wifi_not_connected");
}

/// test_wifi_repair.py::test_repair_health_check_still_runs_for_active_connection
#[test]
fn repair_health_check_still_runs_for_active_connection() {
    let (_dir, app, f) = setup();
    f.connect_home();
    app.wifi.set_health_has_no_connected(false);

    let result = app
        .wifi
        .repair_wifi_connection("manual")
        .expect("no exception");

    assert!(result);
    assert_eq!(ping_count(&f.network), 1);
    assert_eq!(app.wifi.state.lock().last_recovery_result, "not_needed");

    // PARITY: as shipped, the same call raises after the health check.
    app.wifi.set_health_has_no_connected(true);
    assert_eq!(
        app.wifi.repair_wifi_connection("manual"),
        Err(health::no_connected_attribute())
    );
    assert_eq!(ping_count(&f.network), 2);
}

/// `maybeRecoverCurrentConnection`: once a minute, three strikes, and a
/// non-recoverable error never resets the radio.
#[test]
fn recovery_counts_degraded_checks() {
    let (_dir, app, f) = setup();
    f.connect_home();
    f.network.state.lock().internet_ok = false;
    let current = connected_config("HomeNet");

    assert!(
        !app.wifi
            .maybe_recover_current_connection(Some(current.clone()))
            .expect("ok")
    );
    let state = app.wifi.state.lock().clone();
    assert_eq!(state.health_failures, 1);
    assert_eq!(state.last_health_error, "internet_unreachable");
    assert_eq!(state.last_recovery_result, "not_recoverable");
    // Within the minute nothing happens.
    assert!(
        !app.wifi
            .maybe_recover_current_connection(Some(current.clone()))
            .expect("ok")
    );
    assert_eq!(app.wifi.state.lock().health_failures, 1);

    // A recoverable error repairs on the third strike; PARITY: which raises.
    f.network.state.lock().internet_ok = true;
    f.network.state.lock().ping_returncodes = [1, 1, 1].into();
    let strike = || {
        f.network.sleep(60.0);
        app.wifi.state.lock().last_health_error.clear();
        app.wifi
            .maybe_recover_current_connection(Some(current.clone()))
    };
    assert_eq!(strike(), Ok(false));
    assert_eq!(app.wifi.state.lock().health_failures, 2);
    assert_eq!(strike(), Err(health::no_connected_attribute()));
    assert_eq!(app.wifi.state.lock().health_failures, 3);
}

// ---- init and the auto-connect thread ----

fn quiet_host_commands(app: &crate::app::App) -> Arc<RecordingRunner> {
    let runner = RecordingRunner::ok();
    app.hostname.set_runner(runner.clone());
    app.timezone.set_runner(runner.clone());
    runner
}

fn fake_announcer_factory(app: &crate::app::App) -> Arc<FakeAnnouncer> {
    let announcer = Arc::new(FakeAnnouncer::default());
    let handed = announcer.clone();
    app.wifi.set_zeroconf(None);
    app.wifi.set_announcer_factory(Arc::new(move |_| {
        let a: Arc<dyn crate::zeroconf::Announcer> = handed.clone();
        Ok(a)
    }));
    announcer
}

/// `WifiManager.init()` in client mode: renames a machine-named host,
/// names the access point, announces, and starts the auto-connect thread.
#[test]
fn init_in_client_mode() {
    let (_dir, app, f) = setup();
    let runner = quiet_host_commands(&app);
    let announcer = fake_announcer_factory(&app);
    app.config.set(
        keys::SYSTEM,
        keys::DEVICE_IDENTIFIER,
        json!(["fullBodied", "Latte"]),
    );
    app.config
        .set(keys::SYSTEM, keys::MACHINE_SERIAL_NUMBER, json!(1234));

    app.wifi.init().expect("init");

    assert_eq!(
        runner.calls(),
        vec![
            (
                "hostnamectl set-hostname meticulousFullbodiedLatte-1234".to_string(),
                false
            ),
            ("systemctl restart rauc-hawkbit-updater".to_string(), false),
        ]
    );
    assert_eq!(
        app.config.get(keys::WIFI, keys::WIFI_AP_NAME),
        json!("MeticulousFullbodiedLatte")
    );
    assert!(app.wifi.networking_available());
    assert_eq!(f.network.threads(), vec!["WifiAutoConnect"]);
    assert_eq!(announcer.starts.load(Ordering::SeqCst), 1);
    assert!(f.saves.get() >= 1);
}

/// Without NetworkManager `init` still names the access point and starts
/// zeroconf, but touches no radio and starts no thread.
#[test]
fn init_without_networking() {
    let (_dir, app, f) = setup();
    quiet_host_commands(&app);
    let announcer = fake_announcer_factory(&app);
    f.network.state.lock().show_all_error = Some(crate::py::PyError::new(
        "FileNotFoundError",
        "[Errno 2] No such file or directory: 'nmcli'",
    ));
    app.config
        .set(keys::USER, keys::HOSTNAME_OVERRIDE, json!("none"));

    app.wifi.init().expect("init");

    assert!(!app.wifi.networking_available());
    assert!(f.network.threads().is_empty());
    assert_eq!(announcer.starts.load(Ordering::SeqCst), 1);
    assert_eq!(
        app.config.get(keys::WIFI, keys::WIFI_AP_NAME),
        json!("MeticulousEspresso")
    );
}

/// A hostname override renames the machine to exactly that.
#[test]
fn init_applies_the_hostname_override() {
    let (_dir, app, _f) = setup();
    let runner = quiet_host_commands(&app);
    fake_announcer_factory(&app);
    app.config
        .set(keys::USER, keys::HOSTNAME_OVERRIDE, json!("my-machine"));

    app.wifi.init().expect("init");

    assert_eq!(runner.calls()[0].0, "hostnamectl set-hostname my-machine");
}

/// One auto-connect pass while disconnected joins a known network from its
/// saved profile.
#[test]
fn auto_connect_joins_a_known_network() {
    let (_dir, app, f) = setup();
    app.config
        .set(keys::USER, keys::TIMEZONE_SYNC, json!("manual"));
    f.network.add_connection("HomeNet", "wifi");
    f.network.state.lock().scan = vec![wpa2("Neighbor", 80), wpa2("HomeNet", 70)];

    app.wifi.auto_connect_iteration().expect("pass");

    assert_eq!(
        f.network.calls(),
        vec![
            Call::DeviceWifi(Some(true)),
            Call::Up("HomeNet".into(), Some(12))
        ]
    );
    // An automatic join does not hold off the auto-connect thread.
    assert!(!app.wifi.is_auto_connect_suppressed());
}

/// Hotspot mode, a manual connect in progress: the pass does nothing.
#[test]
fn auto_connect_skips_hotspot_mode_and_suppression() {
    let (_dir, app, f) = setup();
    f.start_in_hotspot_mode(&app);
    app.wifi.auto_connect_iteration().expect("pass");
    assert!(f.network.calls().is_empty());

    app.config
        .set(keys::WIFI, keys::WIFI_MODE, json!(WIFI_MODE_CLIENT));
    app.wifi.suppress_auto_connect(90.0, "test");
    app.wifi.auto_connect_iteration().expect("pass");
    assert!(f.network.calls().is_empty());

    // The factory network joins even in hotspot mode during manufacturing.
    app.config.set(
        keys::WIFI,
        keys::WIFI_MODE,
        json!(crate::config::WIFI_MODE_AP),
    );
    app.wifi.set_manufacturing_probe(Arc::new(|| true));
    f.network.sleep(100.0);
    f.network.disconnect_station("disconnected");
    f.network.state.lock().scan = vec![wpa2("MeticulousEPW", 80)];
    app.wifi.auto_connect_iteration().expect("pass");
    assert!(f.network.calls().contains(&Call::WifiConnect(
        "MeticulousEPW".into(),
        Some("23456789".into())
    )));
}

// ---- NetworkManager's view ----

/// `getCurrentConfig()`: addresses, routes, DNS, domains, the connection,
/// and the MAC of a device that is not connected.
#[test]
fn current_config_from_device_details() {
    let (_dir, app, f) = setup();
    {
        let mut state = f.network.state.lock();
        state.devices = vec![
            (
                types::Device {
                    device: "wlan1".into(),
                    device_type: "wifi".into(),
                    state: "disconnected".into(),
                    connection: None,
                },
                DeviceDetails::from_pairs(&[("GENERAL.HWADDR", Some("11:22:33:44:55:66"))]),
            ),
            (
                types::Device {
                    device: "wlan0".into(),
                    device_type: "wifi".into(),
                    state: "connected".into(),
                    connection: Some("HomeNet".into()),
                },
                DeviceDetails::from_pairs(&[
                    ("GENERAL.HWADDR", Some(MAC)),
                    ("GENERAL.CONNECTION", Some("HomeNet")),
                    ("IP4.ADDRESS[1]", Some("192.168.1.42/24")),
                    ("IP4.GATEWAY", Some("192.168.1.1")),
                    (
                        "IP4.ROUTE[1]",
                        Some("dst = 0.0.0.0/0, nh = 192.168.1.1, mt = 600"),
                    ),
                    ("IP4.DNS[1]", Some("192.168.1.1")),
                    ("IP4.DOMAIN[1]", Some("domain_not_set.invalid")),
                    ("IP4.DOMAIN[2]", Some("lan")),
                    ("IP6.ADDRESS[1]", Some("fe80::1/64")),
                    ("IP6.GATEWAY", Some("fe80::ff")),
                    ("IP6.DNS[1]", Some("fd00::1")),
                ]),
            ),
            (
                types::Device {
                    device: "eth0".into(),
                    device_type: "ethernet".into(),
                    state: "connected".into(),
                    connection: Some("Wired".into()),
                },
                DeviceDetails::default(),
            ),
        ];
    }

    let config = app.wifi.get_current_config().expect("config");

    assert!(config.connected);
    assert_eq!(config.connection_name.as_deref(), Some("HomeNet"));
    // The disconnected device came first and set the MAC; the connected
    // one overwrites it.
    assert_eq!(config.mac.as_deref(), Some(MAC));
    assert_eq!(config.domains, vec!["lan"]);
    assert_eq!(
        config.to_json(),
        json!({
            "connected": true,
            "connection_name": "HomeNet",
            "gateway": "192.168.1.1",
            "routes": ["dst = 0.0.0.0/0, nh = 192.168.1.1, mt = 600"],
            "ips": ["192.168.1.42", "fe80::1"],
            "dns": ["192.168.1.1", "fd00::1"],
            "mac": MAC,
            "hostname": HOSTNAME,
        })
    );
}

/// `ZEROCONF_OVERWRITE` replaces NetworkManager's answer.
#[test]
fn zeroconf_overwrite_mocks_the_configuration() {
    let (_dir, app, _f) = setup();
    app.wifi
        .set_zeroconf_overwrite("192.168.2.123/24,MyHostname");

    let config = app.wifi.get_current_config().expect("config");

    assert_eq!(
        config.to_json(),
        json!({
            "connected": true,
            "connection_name": "MeticulousMockConnection",
            "gateway": "192.168.2.0",
            "routes": [],
            "ips": ["192.168.2.123"],
            "dns": ["8.8.8.8"],
            "mac": "AA:BB:CC:FF:FF:FF",
            "hostname": "MyHostname",
        })
    );
}

/// PARITY: a scan for a network that never appears returns the last
/// unfiltered scan.
#[test]
fn targeted_scan_returns_the_last_scan_when_the_target_never_appears() {
    let (_dir, app, f) = setup();
    f.network.state.lock().scan = vec![wpa2("Neighbor", 80)];

    let found = app.wifi.scan_for_networks(2.0, Some(&json!("HomeNet")));

    assert_eq!(found, vec![wpa2("Neighbor", 80)]);
    // A targeted scan leaves the cache alone.
    assert!(app.wifi.scan_cache().is_empty());
    let found = app.wifi.scan_for_networks(2.0, Some(&json!("Neighbor")));
    assert_eq!(found, vec![wpa2("Neighbor", 80)]);
}

/// Legacy `KnownWifis` entries: a profile that already exists takes its
/// type from NetworkManager, a secretless one is rewritten, an open one is
/// migrated without a password.
#[test]
fn legacy_known_wifis_are_scrubbed() {
    let (_dir, app, f) = setup();
    f.network.add_connection("Saved", "wifi");
    f.network
        .state
        .lock()
        .saved_key_mgmt
        .insert("Saved".into(), "sae".into());
    app.config.set(
        keys::WIFI,
        keys::WIFI_KNOWN_WIFIS,
        json!({
            "Saved": "secret",
            "Nopass": {"ssid": "Nopass", "type": "bogus"},
            "Cafe": {"ssid": "Cafe", "type": "OPEN", "password": null},
        }),
    );

    let known = app.wifi.get_known_wifis().expect("known");

    assert_eq!(
        app.config.get(keys::WIFI, keys::WIFI_KNOWN_WIFIS),
        json!({
            "Saved": {"ssid": "Saved", "type": "SAE"},
            "Nopass": {"ssid": "Nopass", "type": "PSK"},
            "Cafe": {"ssid": "Cafe", "type": "OPEN"},
        })
    );
    assert!(f.network.commands_except_type_lookups().contains(&vec![
        "nmcli".to_string(),
        "connection".into(),
        "modify".into(),
        "Cafe".into(),
        "connection.autoconnect".into(),
        "no".into(),
        "802-11-wireless-security.key-mgmt".into(),
        "none".into(),
    ]));
    assert_eq!(known["Saved"], json!({"ssid": "Saved", "type": "SAE"}));
    assert_eq!(known["Cafe"], json!({"ssid": "Cafe", "type": "PSK"}));
    assert_eq!(f.saves.get(), 1);
}

/// The hotspot is never a known network, and deleting it is refused.
#[test]
fn hotspot_profile_is_not_a_known_network() {
    let (_dir, app, f) = setup();
    f.network.add_connection(AP_CONNECTION, "wifi");
    assert!(app.wifi.get_network_manager_wifi_connections().is_empty());
    assert_eq!(app.wifi.delete_wifi(&json!(AP_CONNECTION)), Ok(false));
    assert_eq!(
        app.wifi.delete_wifi(&json!(["x"])).unwrap_err().message,
        "unhashable type: 'list'"
    );
}

/// Before `init`, `_zeroconf.restart()` raises like the Python's `None`.
#[test]
fn zeroconf_restart_before_init_raises() {
    let (_dir, app, f) = setup();
    app.wifi.set_zeroconf(None);
    f.connect_home();
    let error = app
        .wifi
        .connect_to_wifi(&json!({"ssid": "HomeNet", "password": "x"}), "manual")
        .unwrap_err();
    assert_eq!(error.kind, "AttributeError");
    assert_eq!(
        error.message,
        "'NoneType' object has no attribute 'restart'"
    );
}

/// PARITY: python-nmcli cannot join an open network: it passes a `None`
/// password to `subprocess.run`.
#[test]
fn real_nmcli_rejects_an_open_network_join() {
    let runner = RecordingRunner::ok();
    let nmcli = super::nmcli::NmcliCli::with_runner(runner.clone());
    use super::nmcli::Nmcli as _;
    let error = nmcli.device_wifi_connect("Cafe", None).unwrap_err();
    assert_eq!(error.kind, "TypeError");
    assert!(runner.calls().is_empty());
    nmcli
        .device_wifi_connect("Home", Some("pw"))
        .expect("joined");
    assert_eq!(
        runner.calls(),
        vec![(
            "nmcli device wifi connect Home password pw".to_string(),
            false
        )]
    );
}
