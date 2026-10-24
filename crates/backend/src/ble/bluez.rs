//! BlueZ's D-Bus GATT application and LE advertisement, as `bless`'s BlueZ
//! backend exports them (paths, interfaces, properties), with `zbus`.
//!
//! - `/` serves `org.freedesktop.DBus.ObjectManager`; BlueZ reads the
//!   application from it on `RegisterApplication("/")`.
//! - `/org/bluez/<name>/service0001` is the Improv service and
//!   `.../service0001/char000N` its characteristics, in `_build_gatt` order.
//! - `/org/bluez/<name>/advertisement1` is the first advertisement (with the
//!   service UUID), replaced by `.../advertisement0` (with manufacturer
//!   data) on every update.
//! - A pairing agent at `/com/meticulous/ble_agent`, on its own connection,
//!   accepts Just Works pairing.

use std::collections::{BTreeMap, HashMap};
use std::sync::Weak;

use futures::StreamExt;
use zbus::{Connection, OwnedObjectPath, OwnedVariant, Variant, interface};

use super::improv::{
    CAPABILITIES_UUID, ERROR_UUID, RPC_COMMAND_UUID, RPC_RESULT_UUID, SERVICE_UUID, STATUS_UUID,
};
use super::{Advertisement, AdvertisingBackend, GattServer, MACHINE_IDENT_UUID};

const BLUEZ: &str = "org.bluez";
const AGENT_PATH: &str = "/com/meticulous/ble_agent";

/// The characteristics in `_build_gatt` order, with their BlueZ flags.
const CHARACTERISTICS: [(&str, &[&str]); 6] = [
    (MACHINE_IDENT_UUID, &["read"]),
    (STATUS_UUID, &["read", "notify"]),
    (ERROR_UUID, &["read", "notify"]),
    (
        RPC_COMMAND_UUID,
        &["read", "write-without-response", "write"],
    ),
    (RPC_RESULT_UUID, &["read", "notify"]),
    (CAPABILITIES_UUID, &["read"]),
];

pub enum SetupError {
    /// No system bus socket (`FileNotFoundError` in the Python).
    NoBus,
    Failed(String),
}

fn failed(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// The running application: the connection that exports it and what BlueZ
/// knows it by.
pub struct BluezApp {
    conn: Connection,
    adapter: OwnedObjectPath,
    name: String,
    base_path: String,
    char_paths: Vec<(String, String)>,
    advertisements: tokio::sync::Mutex<Vec<String>>,
    _agent: Option<Connection>,
}

/// `BlueZGattApplication.base_path`.
pub fn base_path(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    format!("/org/bluez/{cleaned}")
}

impl BluezApp {
    /// `BlessServerBlueZDBus.setup()`, the pairing agent, powering the
    /// adapter and `add_gatt`.
    pub async fn setup(name: &str, server: Weak<GattServer>) -> Result<BluezApp, SetupError> {
        let conn = match Connection::system().await {
            Ok(conn) => conn,
            Err(zbus::Error::InputOutput(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                return Err(SetupError::NoBus);
            }
            Err(error) => return Err(SetupError::Failed(failed(error))),
        };
        let adapter = find_adapter(&conn, "hci0")
            .await
            .map_err(SetupError::Failed)?;

        let agent = register_pairing_agent().await;
        power_on(&conn, &adapter)
            .await
            .map_err(SetupError::Failed)?;

        let base_path = base_path(name);
        let object_server = conn.object_server();
        object_server
            .at("/", zbus::fdo::ObjectManager)
            .await
            .map_err(|e| SetupError::Failed(failed(e)))?;
        let service_path = format!("{base_path}/service0001");
        object_server
            .at(
                service_path.as_str(),
                Service {
                    uuid: SERVICE_UUID.into(),
                    primary: true,
                },
            )
            .await
            .map_err(|e| SetupError::Failed(failed(e)))?;
        let mut char_paths = Vec::new();
        for (index, (uuid, flags)) in CHARACTERISTICS.iter().enumerate() {
            let path = format!("{service_path}/char{:04}", index + 1);
            let flags: Vec<String> = flags.iter().map(|f| f.to_string()).collect();
            let notifying = flags.iter().any(|f| f == "notify" || f == "indicate");
            object_server
                .at(
                    path.as_str(),
                    Characteristic {
                        uuid: uuid.to_string(),
                        service: OwnedObjectPath::try_from(service_path.as_str())
                            .map_err(|e| SetupError::Failed(failed(e)))?,
                        flags,
                        value: Vec::new(),
                        notifying,
                        server: server.clone(),
                    },
                )
                .await
                .map_err(|e| SetupError::Failed(failed(e)))?;
            char_paths.push((uuid.to_string(), path));
        }

        Ok(BluezApp {
            conn,
            adapter,
            name: name.to_string(),
            base_path,
            char_paths,
            advertisements: tokio::sync::Mutex::new(Vec::new()),
            _agent: agent,
        })
    }

    async fn call<B>(
        &self,
        path: &str,
        interface: &str,
        method: &str,
        body: &B,
    ) -> Result<zbus::Message, String>
    where
        B: serde::Serialize + zbus::DynamicType,
    {
        self.conn
            .call_method(Some(BLUEZ), path, Some(interface), method, body)
            .await
            .map_err(failed)
    }

    /// `BlessServerBlueZDBus.start()`: register the application, then
    /// advertise; and watch devices connect.
    pub async fn start(&self) -> Result<(), String> {
        let empty: HashMap<String, OwnedVariant> = HashMap::new();
        let root = OwnedObjectPath::try_from("/").map_err(failed)?;
        let registered = self
            .call(
                self.adapter.as_str(),
                "org.bluez.GattManager1",
                "RegisterApplication",
                &(root, empty.clone()),
            )
            .await;
        if let Err(error) = registered {
            tracing::error!("Failed to start GATT server: {error}");
            return Err(error);
        }

        // `start_advertising`.
        set_adapter_property(
            &self.conn,
            &self.adapter,
            "Alias",
            Variant::from(self.name.as_str()),
        )
        .await?;
        let mut ads = self.advertisements.lock().await;
        let advertisement = Advertisement {
            path: format!("{}/advertisement{}", self.base_path, ads.len() + 1),
            local_name: self.name.clone(),
            // Only the first service.
            service_uuids: vec![SERVICE_UUID.to_string()],
            manufacturer_data: None,
        };
        let path = advertisement.path.clone();
        ads.push(path.clone());
        self.export_advertisement(advertisement).await?;
        let path = OwnedObjectPath::try_from(path.as_str()).map_err(failed)?;
        self.call(
            self.adapter.as_str(),
            "org.bluez.LEAdvertisingManager1",
            "RegisterAdvertisement",
            &(path, empty),
        )
        .await?;
        drop(ads);

        self.watch_devices().await?;
        Ok(())
    }

    /// `BlessServerBlueZDBus.stop()`.
    pub async fn stop(&self) -> Result<(), String> {
        set_adapter_property(&self.conn, &self.adapter, "Alias", Variant::from("")).await?;
        if let Some(path) = self.advertisements.lock().await.pop() {
            let object = OwnedObjectPath::try_from(path.as_str()).map_err(failed)?;
            self.call(
                self.adapter.as_str(),
                "org.bluez.LEAdvertisingManager1",
                "UnregisterAdvertisement",
                &(object,),
            )
            .await?;
            let _ = self
                .conn
                .object_server()
                .remove::<AdvertisementObject, _>(path.as_str())
                .await;
        }
        let root = OwnedObjectPath::try_from("/").map_err(failed)?;
        self.call(
            self.adapter.as_str(),
            "org.bluez.GattManager1",
            "UnregisterApplication",
            &(root,),
        )
        .await?;
        Ok(())
    }

    async fn export_advertisement(&self, ad: Advertisement) -> Result<(), String> {
        let path = ad.path.clone();
        let object = AdvertisementObject {
            path: ad.path,
            local_name: ad.local_name,
            service_uuids: ad.service_uuids,
            manufacturer_data: ad.manufacturer_data,
        };
        match self.conn.object_server().at(path.as_str(), object).await {
            Ok(None) => Ok(()),
            Ok(Some(_)) => Err(format!("An object is already exported at {path}")),
            Err(error) => Err(failed(error)),
        }
    }

    /// Log BLE clients connecting and disconnecting (`Device1` changes).
    async fn watch_devices(&self) -> Result<(), String> {
        let rule = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender(BLUEZ)
            .interface("org.freedesktop.DBus.Properties")
            .member("PropertiesChanged")
            .arg(0, "org.bluez.Device1")
            .build()
            .map_err(failed)?;
        let mut stream = zbus::MessageStream::for_match_rule(rule, &self.conn, None)
            .await
            .map_err(failed)?;
        tokio::spawn(async move {
            while let Some(Ok(message)) = stream.next().await {
                let body = message.body();
                let Ok((interface, changed, _)) =
                    body.deserialize::<(String, HashMap<String, OwnedVariant>, Vec<String>)>()
                else {
                    continue;
                };
                if interface != "org.bluez.Device1" {
                    continue;
                }
                // Only the name: the object path and Address carry the MAC.
                let name = changed
                    .get("Name")
                    .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
                    .unwrap_or_else(|| "unknown".into());
                if let Some(connected) = changed
                    .get("Connected")
                    .and_then(|v| bool::try_from(v).ok())
                {
                    if connected {
                        tracing::info!("BLE client connected (name={name})");
                    } else {
                        tracing::info!("BLE client disconnected (name={name})");
                    }
                }
                if let Some(reason) = changed.get("DisconnectReason") {
                    tracing::info!("BLE disconnect reason: {reason:?} (name={name})");
                }
            }
        });
        Ok(())
    }

    pub fn char_path(&self, uuid: &str) -> Option<&str> {
        self.char_paths
            .iter()
            .find(|(u, _)| u == uuid)
            .map(|(_, p)| p.as_str())
    }

    /// `update_value(service, characteristic)` after setting its value: the
    /// `Value` property changes and subscribers are notified.
    pub async fn update_value(&self, uuid: &str, value: Vec<u8>) -> Result<(), String> {
        let Some(path) = self.char_path(uuid) else {
            return Err(format!("no characteristic {uuid}"));
        };
        let iface = self
            .conn
            .object_server()
            .interface::<_, Characteristic>(path)
            .await
            .map_err(failed)?;
        iface.get_mut().await.value = value;
        iface
            .get()
            .await
            .value_changed(iface.signal_emitter())
            .await
            .map_err(failed)
    }
}

impl AdvertisingBackend for BluezApp {
    fn base_path(&self) -> String {
        self.base_path.clone()
    }

    async fn take_last(&self) -> Option<String> {
        self.advertisements.lock().await.pop()
    }

    async fn push(&self, path: String) {
        self.advertisements.lock().await.push(path);
    }

    async fn unregister(&self, path: &str) -> Result<(), String> {
        let object = OwnedObjectPath::try_from(path).map_err(failed)?;
        self.call(
            self.adapter.as_str(),
            "org.bluez.LEAdvertisingManager1",
            "UnregisterAdvertisement",
            &(object,),
        )
        .await
        .map(|_| ())
    }

    async fn register(&self, path: &str) -> Result<(), String> {
        let object = OwnedObjectPath::try_from(path).map_err(failed)?;
        let empty: HashMap<String, OwnedVariant> = HashMap::new();
        self.call(
            self.adapter.as_str(),
            "org.bluez.LEAdvertisingManager1",
            "RegisterAdvertisement",
            &(object, empty),
        )
        .await
        .map(|_| ())
    }

    async fn export(&self, ad: Advertisement) -> Result<(), String> {
        self.export_advertisement(ad).await
    }

    async fn unexport(&self, path: &str) -> Result<(), String> {
        self.conn
            .object_server()
            .remove::<AdvertisementObject, _>(path)
            .await
            .map(|_| ())
            .map_err(failed)
    }
}

/// `find_adapter(bus, "hci0")`: the first BlueZ object with a GATT manager
/// whose path mentions the adapter.
async fn find_adapter(conn: &Connection, adapter: &str) -> Result<OwnedObjectPath, String> {
    type Managed = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedVariant>>>;
    let reply = conn
        .call_method(
            Some(BLUEZ),
            "/",
            Some("org.freedesktop.DBus.ObjectManager"),
            "GetManagedObjects",
            &(),
        )
        .await
        .map_err(failed)?;
    let objects: Managed = reply.body().deserialize().map_err(failed)?;
    let adapters: BTreeMap<String, OwnedObjectPath> = objects
        .into_iter()
        .filter(|(_, interfaces)| interfaces.contains_key("org.bluez.GattManager1"))
        .map(|(path, _)| (path.as_str().to_string(), path))
        .collect();
    adapters
        .into_values()
        .find(|path| path.as_str().contains(adapter))
        .ok_or_else(|| format!("No adapter named {adapter} found"))
}

async fn set_adapter_property(
    conn: &Connection,
    adapter: &OwnedObjectPath,
    name: &str,
    value: Variant<'_>,
) -> Result<(), String> {
    conn.call_method(
        Some(BLUEZ),
        adapter.as_str(),
        Some("org.freedesktop.DBus.Properties"),
        "Set",
        &("org.bluez.Adapter1", name, value),
    )
    .await
    .map(|_| ())
    .map_err(failed)
}

/// Power the adapter on when it is off.
async fn power_on(conn: &Connection, adapter: &OwnedObjectPath) -> Result<(), String> {
    let powered = async {
        let reply = conn
            .call_method(
                Some(BLUEZ),
                adapter.as_str(),
                Some("org.freedesktop.DBus.Properties"),
                "Get",
                &("org.bluez.Adapter1", "Powered"),
            )
            .await?;
        let value: OwnedVariant = reply.body().deserialize()?;
        let powered = bool::try_from(&value).unwrap_or(false);
        if !powered {
            tracing::info!("bluetooth device is not powered, powering now!");
            conn.call_method(
                Some(BLUEZ),
                adapter.as_str(),
                Some("org.freedesktop.DBus.Properties"),
                "Set",
                &("org.bluez.Adapter1", "Powered", Variant::from(true)),
            )
            .await?;
        }
        Ok::<(), zbus::Error>(())
    };
    powered
        .await
        .map_err(|error| format!("Failed to power device. DBusError: {error}"))
}

/// `register_pairing_agent()`: without an agent bluetoothd answers iOS's
/// pairing request with "Pairing not supported" and drops the connection.
async fn register_pairing_agent() -> Option<Connection> {
    let result = async {
        let conn = Connection::system().await?;
        conn.object_server().at(AGENT_PATH, Agent).await?;
        let path = OwnedObjectPath::try_from(AGENT_PATH)?;
        conn.call_method(
            Some(BLUEZ),
            "/org/bluez",
            Some("org.bluez.AgentManager1"),
            "RegisterAgent",
            &(path.clone(), "KeyboardDisplay"),
        )
        .await?;
        conn.call_method(
            Some(BLUEZ),
            "/org/bluez",
            Some("org.bluez.AgentManager1"),
            "RequestDefaultAgent",
            &(path,),
        )
        .await?;
        Ok::<Connection, zbus::Error>(conn)
    }
    .await;
    match result {
        Ok(conn) => {
            tracing::info!("[BLE Agent] Registered NoInputNoOutput pairing agent");
            Some(conn)
        }
        Err(error) => {
            let kind = match error {
                zbus::Error::MethodError(..) | zbus::Error::FDO(_) => "DBusError",
                _ => "Exception",
            };
            tracing::warn!("[BLE Agent] Failed to register pairing agent: {kind}");
            None
        }
    }
}

/// `org.bluez.GattService1`.
struct Service {
    uuid: String,
    primary: bool,
}

#[interface(name = "org.bluez.GattService1")]
impl Service {
    #[zbus(property, name = "UUID")]
    fn uuid(&self) -> String {
        self.uuid.clone()
    }

    #[zbus(property)]
    fn primary(&self) -> bool {
        self.primary
    }
}

/// `org.bluez.GattCharacteristic1`.
pub struct Characteristic {
    uuid: String,
    service: OwnedObjectPath,
    flags: Vec<String>,
    value: Vec<u8>,
    notifying: bool,
    server: Weak<GattServer>,
}

fn dbus_failure(message: impl Into<String>) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed(message.into())
}

#[interface(name = "org.bluez.GattCharacteristic1")]
impl Characteristic {
    #[zbus(property, name = "UUID")]
    fn uuid(&self) -> String {
        self.uuid.clone()
    }

    #[zbus(property)]
    fn service(&self) -> OwnedObjectPath {
        self.service.clone()
    }

    #[zbus(property)]
    fn value(&self) -> Vec<u8> {
        self.value.clone()
    }

    #[zbus(property)]
    fn set_value(&mut self, value: Vec<u8>) {
        self.value = value;
    }

    #[zbus(property)]
    fn notifying(&self) -> bool {
        self.notifying
    }

    #[zbus(property)]
    fn flags(&self) -> Vec<String> {
        self.flags.clone()
    }

    async fn read_value(
        &self,
        _options: HashMap<String, OwnedVariant>,
    ) -> zbus::fdo::Result<Vec<u8>> {
        let server = self
            .server
            .upgrade()
            .ok_or_else(|| dbus_failure("server gone"))?;
        let uuid = self.uuid.clone();
        tokio::task::spawn_blocking(move || server.read_request(&uuid))
            .await
            .map_err(|e| dbus_failure(e.to_string()))?
            .map_err(|e| dbus_failure(format!("{}: {}", e.kind, e.message)))
    }

    async fn write_value(
        &self,
        value: Vec<u8>,
        _options: HashMap<String, OwnedVariant>,
    ) -> zbus::fdo::Result<()> {
        let server = self
            .server
            .upgrade()
            .ok_or_else(|| dbus_failure("server gone"))?;
        let uuid = self.uuid.clone();
        let blocking = server.clone();
        let notifications =
            tokio::task::spawn_blocking(move || blocking.write_request(&uuid, &value))
                .await
                .map_err(|e| dbus_failure(e.to_string()))?
                .map_err(|e| dbus_failure(format!("{}: {}", e.kind, e.message)))?;
        if let Some(app) = server.app.get() {
            for (target, packet) in notifications {
                if let Err(error) = app.update_value(&target, packet).await {
                    tracing::warn!("Updating characteristic return status=False ({error})");
                }
            }
        }
        Ok(())
    }

    async fn start_notify(&self) -> zbus::fdo::Result<()> {
        if let Some(server) = self.server.upgrade() {
            server.start_notify(&self.uuid);
        }
        Ok(())
    }

    async fn stop_notify(&self) -> zbus::fdo::Result<()> {
        match self.server.upgrade() {
            Some(server) => server.stop_notify(&self.uuid).map_err(dbus_failure),
            None => Ok(()),
        }
    }
}

/// `org.bluez.LEAdvertisement1` (`BlueZLEAdvertisement`).
pub struct AdvertisementObject {
    path: String,
    local_name: String,
    service_uuids: Vec<String>,
    manufacturer_data: Option<Vec<u8>>,
}

#[interface(name = "org.bluez.LEAdvertisement1")]
impl AdvertisementObject {
    fn release(&self) {
        tracing::info!("{}: Released!", self.path);
    }

    #[zbus(property, name = "Type")]
    fn kind(&self) -> String {
        "peripheral".into()
    }

    #[zbus(property, name = "ServiceUUIDs")]
    fn service_uuids(&self) -> Vec<String> {
        self.service_uuids.clone()
    }

    #[zbus(property)]
    fn manufacturer_data(&self) -> HashMap<u16, OwnedVariant> {
        let mut data = HashMap::new();
        if let Some(bytes) = &self.manufacturer_data
            && let Ok(value) = OwnedVariant::try_from(Variant::from(bytes.clone()))
        {
            data.insert(0xFFFF, value);
        }
        data
    }

    #[zbus(property)]
    fn service_data(&self) -> HashMap<String, OwnedVariant> {
        HashMap::new()
    }

    #[zbus(property)]
    fn tx_power(&self) -> i16 {
        20
    }

    #[zbus(property)]
    fn max_interval(&self) -> u32 {
        100
    }

    #[zbus(property)]
    fn min_interval(&self) -> u32 {
        100
    }

    #[zbus(property)]
    fn local_name(&self) -> String {
        self.local_name.clone()
    }
}

/// `NoInputNoOutputAgent` (registered as "KeyboardDisplay", as the Python
/// does).
struct Agent;

#[interface(name = "org.bluez.Agent1")]
impl Agent {
    fn release(&self) {
        tracing::info!("[BLE Agent] Released");
    }

    fn request_confirmation(&self, device: OwnedObjectPath, _passkey: u32) {
        tracing::info!(
            "[BLE Agent] Auto-confirming pairing for {}",
            device.as_str()
        );
    }

    fn authorize_service(&self, device: OwnedObjectPath, uuid: String) {
        tracing::info!(
            "[BLE Agent] Authorizing service {uuid} for {}",
            device.as_str()
        );
    }

    fn cancel(&self) {
        tracing::info!("[BLE Agent] Pairing cancelled");
    }
}

#[cfg(test)]
mod tests;
