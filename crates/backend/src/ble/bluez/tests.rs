//! The BlueZ objects as BlueZ would read them: paths like bless's, and, on
//! a private bus when `dbus-daemon` is installed, the interface and property
//! names, the D-Bus types, and working ReadValue/WriteValue/StartNotify.

use std::io::BufRead;
use std::sync::Arc;

use super::*;
use crate::ble::tests::FakeWifi;

#[test]
fn object_paths_like_bless() {
    assert_eq!(
        base_path("MeticulousAromatic-Espresso!"),
        "/org/bluez/MeticulousAromaticEspresso"
    );
    assert_eq!(
        CHARACTERISTICS[3].1,
        &["read", "write-without-response", "write"]
    );
    assert_eq!(format!("char{:04}", 1), "char0001");
}

/// The pairing code goes out in a GATT notification, which BlueZ sends to
/// every connected central that subscribed, so the server counts who is
/// connected first. Without this filter a central BlueZ only remembers
/// (bonded, out of range) or one on another adapter would count, and the
/// code would be held back from a phone that is alone with the machine, or
/// sent while someone else listens. The shape is BlueZ's: `Device1` objects
/// below the adapter's path with a `Connected` property.
#[test]
fn connected_devices_are_this_adapters_connected_centrals() {
    let device = |connected: Option<bool>| {
        let mut properties = HashMap::new();
        if let Some(connected) = connected {
            properties.insert("Connected".to_string(), variant(connected).expect("bool"));
        }
        HashMap::from([("org.bluez.Device1".to_string(), properties)])
    };
    let objects: Managed = HashMap::from([
        (
            path("/org/bluez/hci0"),
            HashMap::from([("org.bluez.Adapter1".to_string(), HashMap::new())]),
        ),
        (
            path("/org/bluez/hci0/dev_00_00_00_00_00_01"),
            device(Some(true)),
        ),
        (
            path("/org/bluez/hci0/dev_00_00_00_00_00_02"),
            device(Some(false)),
        ),
        (path("/org/bluez/hci0/dev_00_00_00_00_00_03"), device(None)),
        (
            path("/org/bluez/hci1/dev_00_00_00_00_00_04"),
            device(Some(true)),
        ),
    ]);

    // No per-transport interfaces anywhere: `Device1.Connected` decides.
    assert_eq!(
        connected_devices(&objects, "/org/bluez/hci0"),
        (
            vec!["/org/bluez/hci0/dev_00_00_00_00_00_01".to_string()],
            false
        )
    );
}

/// Only a device's LE link makes it a central. The image runs PulseAudio's
/// Bluetooth module and `obexd`, so a phone or speaker can hold a
/// Bluetooth Classic link (audio, file transfer); counted, it would be
/// disconnected, or keep every phone from getting its pairing code, though
/// BlueZ never sends it a GATT notification. Where BlueZ exports
/// `org.bluez.Bearer.LE1` and `org.bluez.Bearer.BREDR1` (each with its own
/// `Connected`, `Device1.Connected` true for either), only a connected LE
/// bearer counts. The shape is BlueZ's `GetManagedObjects` reply with those
/// interfaces on the device objects.
#[test]
fn only_le_links_count_as_centrals_where_bluez_tells_them_apart() {
    let connected =
        |value: bool| HashMap::from([("Connected".to_string(), variant(value).expect("bool"))]);
    let device = |le: Option<bool>, classic: Option<bool>| {
        let mut interfaces = HashMap::from([(
            "org.bluez.Device1".to_string(),
            connected(le == Some(true) || classic == Some(true)),
        )]);
        if let Some(le) = le {
            interfaces.insert("org.bluez.Bearer.LE1".to_string(), connected(le));
        }
        if let Some(classic) = classic {
            interfaces.insert("org.bluez.Bearer.BREDR1".to_string(), connected(classic));
        }
        interfaces
    };
    let objects: Managed = HashMap::from([
        (
            path("/org/bluez/hci0"),
            HashMap::from([("org.bluez.Adapter1".to_string(), HashMap::new())]),
        ),
        // An LE central.
        (
            path("/org/bluez/hci0/dev_00_00_00_00_00_01"),
            device(Some(true), Some(false)),
        ),
        // A Classic link only (audio): not a central.
        (
            path("/org/bluez/hci0/dev_00_00_00_00_00_02"),
            device(Some(false), Some(true)),
        ),
        // A Classic-only device BlueZ gives no LE bearer: not a central.
        (
            path("/org/bluez/hci0/dev_00_00_00_00_00_03"),
            device(None, Some(true)),
        ),
        // Both links up: a central.
        (
            path("/org/bluez/hci0/dev_00_00_00_00_00_04"),
            device(Some(true), Some(true)),
        ),
        // Remembered, nothing connected.
        (
            path("/org/bluez/hci0/dev_00_00_00_00_00_05"),
            device(Some(false), Some(false)),
        ),
        // An LE central on another adapter.
        (
            path("/org/bluez/hci1/dev_00_00_00_00_00_06"),
            device(Some(true), None),
        ),
    ]);

    assert_eq!(
        connected_devices(&objects, "/org/bluez/hci0"),
        (
            vec![
                "/org/bluez/hci0/dev_00_00_00_00_00_01".to_string(),
                "/org/bluez/hci0/dev_00_00_00_00_00_04".to_string(),
            ],
            true
        )
    );
}

/// BlueZ lays the characteristics out in the order of the application's
/// GetManagedObjects reply: it must be the export order, on every start, or
/// a phone that kept the table it discovered reads the wrong attributes.
#[test]
fn application_lists_objects_in_export_order() {
    let service = "/org/bluez/MeticulousEspresso/service0001";
    let listed: Vec<String> = GattApplication::new(service)
        .objects()
        .keys()
        .map(|p| p.as_str().to_string())
        .collect();
    let mut expected = vec![service.to_string()];
    expected.extend((1..=CHARACTERISTICS.len()).map(|i| format!("{service}/char{i:04}")));
    assert_eq!(listed, expected);
}

/// A `dbus-daemon` that is killed when dropped, also when a test panics.
struct Daemon(std::process::Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A private bus, when `dbus-daemon` is installed and prints its address
/// within 10 s.
fn private_bus() -> Option<(Daemon, String)> {
    let mut daemon = Daemon(
        std::process::Command::new("dbus-daemon")
            .args(["--session", "--print-address", "--nofork", "--nopidfile"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .ok()?,
    );
    let stdout = daemon.0.stdout.take()?;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let read = std::io::BufReader::new(stdout).read_line(&mut line);
        let _ = sender.send(read.map(|_| line));
    });
    match receiver.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(Ok(line)) if !line.trim().is_empty() => Some((daemon, line.trim().to_string())),
        _ => None,
    }
}

fn path(text: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(text).expect("object path")
}

fn sorted_keys(map: &HashMap<String, OwnedVariant>) -> Vec<String> {
    let mut keys: Vec<String> = map.keys().cloned().collect();
    keys.sort();
    keys
}

/// On the multi-threaded runtime the backend serves these objects on, and
/// bounded: a hang fails the test instead of the CI job.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn objects_on_a_private_bus() {
    let Some((daemon, address)) = private_bus() else {
        eprintln!("dbus-daemon not available; skipped");
        return;
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(60),
        exported_objects(address),
    )
    .await
    .expect("the D-Bus exchange did not finish within 60 s");
    drop(daemon);
}

async fn exported_objects(address: String) {
    let (_dir, app) = crate::testing::app();
    let gatt = GattServer::new(
        FakeWifi::new(),
        app.hostname.clone(),
        app.config.clone(),
        app.notifications.clone(),
        app.auth.clone(),
        8080,
    );
    // Built as `BluezApp::setup` builds it: with `object_server().at("/")`
    // after `build()`, GetManagedObjects below was lost about once in 150
    // runs on two loaded cores and the test hung.
    let server = zbus::connection::Builder::address(address.as_str())
        .serve_at("/", zbus::fdo::ObjectManager)
        .build()
        .await
        .expect("server connection");
    let client = zbus::connection::Builder::address(address.as_str())
        .build()
        .await
        .expect("client connection");
    let objects = server.object_server();
    let base = base_path("MeticulousEspresso");
    let service_path = format!("{base}/service0001");
    objects
        .at(
            service_path.as_str(),
            Service {
                uuid: SERVICE_UUID.into(),
                primary: true,
                handle: SERVICE_HANDLE,
            },
        )
        .await
        .expect("service");
    for (index, (uuid, flags)) in CHARACTERISTICS.iter().enumerate() {
        let char_path = format!("{service_path}/char{:04}", index + 1);
        objects
            .at(
                char_path.as_str(),
                Characteristic {
                    uuid: uuid.to_string(),
                    service: path(&service_path),
                    flags: flags.iter().map(|f| f.to_string()).collect(),
                    value: Vec::new(),
                    notifying: flags.contains(&"notify"),
                    server: Arc::downgrade(&gatt),
                },
            )
            .await
            .expect("characteristic");
    }
    objects
        .at(base.as_str(), GattApplication::new(&service_path))
        .await
        .expect("application");
    let ad_path = format!("{base}/advertisement0");
    objects
        .at(
            ad_path.as_str(),
            AdvertisementObject {
                path: ad_path.clone(),
                local_name: "MeticulousEspresso".into(),
                service_uuids: Vec::new(),
                manufacturer_data: Some(vec![0x01, 192, 168, 1, 100]),
            },
        )
        .await
        .expect("advertisement");

    let owner = server.unique_name().expect("unique name").to_string();
    let reply = client
        .call_method(
            Some(owner.as_str()),
            "/",
            Some("org.freedesktop.DBus.ObjectManager"),
            "GetManagedObjects",
            &(),
        )
        .await
        .expect("managed objects");
    let managed: Managed = reply.body().deserialize().expect("decode");

    let service = &managed[&path(&service_path)]["org.bluez.GattService1"];
    assert_eq!(sorted_keys(service), ["Handle", "Primary", "UUID"]);
    assert_eq!(service["Handle"].value_signature().to_string(), "q");
    assert_eq!(service["UUID"].value_signature().to_string(), "s");
    assert_eq!(service["Primary"].value_signature().to_string(), "b");

    let rpc_path = format!("{service_path}/char0004");
    let rpc = &managed[&path(&rpc_path)]["org.bluez.GattCharacteristic1"];
    assert_eq!(
        sorted_keys(rpc),
        ["Flags", "Notifying", "Service", "UUID", "Value"]
    );
    assert_eq!(rpc["Flags"].value_signature().to_string(), "as");
    assert_eq!(rpc["Value"].value_signature().to_string(), "ay");
    assert_eq!(rpc["Service"].value_signature().to_string(), "o");
    assert_eq!(rpc["Notifying"].value_signature().to_string(), "b");

    let ad = &managed[&path(&ad_path)]["org.bluez.LEAdvertisement1"];
    assert_eq!(
        sorted_keys(ad),
        [
            "LocalName",
            "ManufacturerData",
            "MaxInterval",
            "MinInterval",
            "ServiceData",
            "ServiceUUIDs",
            "TxPower",
            "Type"
        ]
    );
    assert_eq!(
        ad["ManufacturerData"].value_signature().to_string(),
        "a{qv}"
    );
    assert_eq!(ad["ServiceData"].value_signature().to_string(), "a{sv}");
    assert_eq!(ad["ServiceUUIDs"].value_signature().to_string(), "as");
    assert_eq!(ad["TxPower"].value_signature().to_string(), "n");
    assert_eq!(ad["MaxInterval"].value_signature().to_string(), "u");
    assert_eq!(ad["Type"].value_signature().to_string(), "s");

    // What BlueZ reads on RegisterApplication(base): the service and its six
    // characteristics only, the service at its fixed handle, and the same
    // properties the objects themselves report.
    let reply = client
        .call_method(
            Some(owner.as_str()),
            base.as_str(),
            Some("org.freedesktop.DBus.ObjectManager"),
            "GetManagedObjects",
            &(),
        )
        .await
        .expect("application objects");
    let application: Managed = reply.body().deserialize().expect("decode");
    let mut listed: Vec<String> = application.keys().map(|p| p.as_str().to_string()).collect();
    listed.sort();
    let mut expected = vec![service_path.clone()];
    expected.extend((1..=6).map(|i| format!("{service_path}/char{i:04}")));
    assert_eq!(listed, expected);
    let listed_service = &application[&path(&service_path)]["org.bluez.GattService1"];
    assert_eq!(
        u16::try_from(&listed_service["Handle"]).expect("u16"),
        SERVICE_HANDLE
    );
    for (key, value) in service {
        assert_eq!(
            listed_service[key].value_signature(),
            value.value_signature(),
            "{key}"
        );
    }
    let listed_rpc = &application[&path(&rpc_path)]["org.bluez.GattCharacteristic1"];
    assert_eq!(sorted_keys(listed_rpc), sorted_keys(rpc));
    assert_eq!(
        Vec::<String>::try_from(listed_rpc["Flags"].try_clone().expect("clone")).expect("as"),
        ["read", "write-without-response", "write"]
    );

    let empty: HashMap<String, OwnedVariant> = HashMap::new();
    let ident = client
        .call_method(
            Some(owner.as_str()),
            format!("{service_path}/char0001").as_str(),
            Some("org.bluez.GattCharacteristic1"),
            "ReadValue",
            &(empty.clone(),),
        )
        .await
        .expect("read");
    let ident: Vec<u8> = ident.body().deserialize().expect("bytes");
    assert!(
        String::from_utf8(ident)
            .expect("utf8")
            .starts_with("MeticulousEspresso,meticulous,")
    );

    client
        .call_method(
            Some(owner.as_str()),
            rpc_path.as_str(),
            Some("org.bluez.GattCharacteristic1"),
            "WriteValue",
            &(vec![0x04u8], empty.clone()),
        )
        .await
        .expect("write");
    let result = client
        .call_method(
            Some(owner.as_str()),
            format!("{service_path}/char0005").as_str(),
            Some("org.bluez.GattCharacteristic1"),
            "ReadValue",
            &(empty,),
        )
        .await
        .expect("read result");
    let result: Vec<u8> = result.body().deserialize().expect("bytes");
    assert_eq!(&result[..2], &[0x04, 9]);

    client
        .call_method(
            Some(owner.as_str()),
            format!("{service_path}/char0002").as_str(),
            Some("org.bluez.GattCharacteristic1"),
            "StartNotify",
            &(),
        )
        .await
        .expect("subscribe");
    assert_eq!(*gatt.subscribed.lock(), vec![STATUS_UUID.to_string()]);
}
