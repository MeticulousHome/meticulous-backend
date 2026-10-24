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

/// A private bus, when `dbus-daemon` is installed.
fn private_bus() -> Option<(std::process::Child, String)> {
    let mut child = std::process::Command::new("dbus-daemon")
        .args(["--session", "--print-address", "--nofork", "--nopidfile"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .ok()?;
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take()?)
        .read_line(&mut line)
        .ok()?;
    Some((child, line.trim().to_string()))
}

fn path(text: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(text).expect("object path")
}

type Managed = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedVariant>>>;

fn sorted_keys(map: &HashMap<String, OwnedVariant>) -> Vec<String> {
    let mut keys: Vec<String> = map.keys().cloned().collect();
    keys.sort();
    keys
}

#[tokio::test]
async fn objects_on_a_private_bus() {
    let Some((mut daemon, address)) = private_bus() else {
        eprintln!("dbus-daemon not available; skipped");
        return;
    };
    let (_dir, app) = crate::testing::app();
    let gatt = GattServer::new(
        FakeWifi::new(),
        app.hostname.clone(),
        app.config.clone(),
        8080,
    );
    let server = zbus::connection::Builder::address(address.as_str())
        .build()
        .await
        .expect("server connection");
    let client = zbus::connection::Builder::address(address.as_str())
        .build()
        .await
        .expect("client connection");
    let objects = server.object_server();
    objects.at("/", zbus::fdo::ObjectManager).await.expect("om");
    let base = base_path("MeticulousEspresso");
    let service_path = format!("{base}/service0001");
    objects
        .at(
            service_path.as_str(),
            Service {
                uuid: SERVICE_UUID.into(),
                primary: true,
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
    assert_eq!(sorted_keys(service), ["Primary", "UUID"]);
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

    let _ = daemon.kill();
    let _ = daemon.wait();
}
