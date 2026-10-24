//! `dbus_monitor.py` and `dbus_client.py`: signals on the system bus from
//! RAUC (install progress, errors, completion), rauc-hawkbit-updater
//! (download progress and errors) and meticulous-machine's USB handlers
//! (new USB stick, recovery update). They drive the OS update status and the
//! notifications a USB recovery update shows.
//!
//! Like `AsyncDBUSClient`, every subscription adds a match rule and a
//! callback keyed by (interface, member); events are handled one at a time,
//! in arrival order, and a key subscribed twice is delivered twice.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use parking_lot::Mutex;
use serde_json::Value;

use crate::config::{ConfigStore, keys};
use crate::host::Host;
use crate::images::FLASHING_NOTIFICATION_IMAGE;
use crate::notifications::{Notification, NotificationManager, responses};
use crate::os_update::{OsStatus, OsUpdateStatus};

const POSSIBLE_UPDATER: &str = "/tmp/possible_updater";

/// A D-Bus value as GLib hands it to Python callbacks.
#[derive(Debug, Clone, PartialEq)]
pub enum DValue {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    /// Arrays and structures, Python lists and tuples.
    List(Vec<DValue>),
    Dict(Vec<(DValue, DValue)>),
}

impl DValue {
    /// `str(value)` / an f-string.
    pub fn python_str(&self) -> String {
        match self {
            DValue::Int(i) => i.to_string(),
            DValue::Float(f) => pyfmt::repr_f64(*f),
            DValue::Str(s) => s.clone(),
            DValue::Bool(b) => if *b { "True" } else { "False" }.to_string(),
            other => other.python_repr(),
        }
    }

    pub fn python_repr(&self) -> String {
        match self {
            DValue::Str(s) => crate::usb::python_repr(&Value::String(s.clone())),
            DValue::List(items) => {
                let inner: Vec<String> = items.iter().map(DValue::python_repr).collect();
                if items.len() == 1 {
                    format!("({},)", inner[0])
                } else {
                    format!("({})", inner.join(", "))
                }
            }
            DValue::Dict(entries) => format!(
                "{{{}}}",
                entries
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k.python_repr(), v.python_repr()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            other => other.python_str(),
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            DValue::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The value as a Python number, for the OS update progress.
    fn as_number(&self) -> Option<f64> {
        match self {
            DValue::Int(i) => Some(*i as f64),
            DValue::Float(f) => Some(*f),
            DValue::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }
}

fn from_variant(value: &zbus::wire::Variant<'_>) -> DValue {
    use zbus::wire::Variant;
    match value {
        Variant::U8(v) => DValue::Int(i64::from(*v)),
        Variant::Bool(v) => DValue::Bool(*v),
        Variant::I16(v) => DValue::Int(i64::from(*v)),
        Variant::U16(v) => DValue::Int(i64::from(*v)),
        Variant::I32(v) => DValue::Int(i64::from(*v)),
        Variant::U32(v) => DValue::Int(i64::from(*v)),
        Variant::I64(v) => DValue::Int(*v),
        Variant::U64(v) => DValue::Int(i64::try_from(*v).unwrap_or(i64::MAX)),
        Variant::F64(v) => DValue::Float(*v),
        Variant::Str(v) => DValue::Str(v.as_str().to_string()),
        Variant::Signature(v) => DValue::Str(v.to_string()),
        Variant::ObjectPath(v) => DValue::Str(v.as_str().to_string()),
        Variant::Variant(inner) => from_variant(inner),
        Variant::Array(items) => DValue::List(items.inner().iter().map(from_variant).collect()),
        Variant::Dict(dict) => DValue::Dict(
            dict.iter()
                .map(|(k, v)| (from_variant(k), from_variant(v)))
                .collect(),
        ),
        Variant::Structure(fields) => {
            DValue::List(fields.fields().iter().map(from_variant).collect())
        }
        #[allow(unreachable_patterns)]
        _ => DValue::Str(String::new()),
    }
}

/// A signal as `on_dbus_event` queues it.
#[derive(Debug, Clone, PartialEq)]
pub struct DbusEvent {
    pub interface: String,
    pub member: String,
    pub args: Vec<DValue>,
}

impl DbusEvent {
    pub fn new(interface: &str, member: &str, args: Vec<DValue>) -> DbusEvent {
        DbusEvent {
            interface: interface.to_string(),
            member: member.to_string(),
            args,
        }
    }

    fn from_message(message: &zbus::Message) -> Option<DbusEvent> {
        let header = message.header();
        if header.message_type() != zbus::message::Type::Signal {
            return None;
        }
        let interface = header.interface()?.to_string();
        let member = header.member()?.to_string();
        let body = message.body();
        let args = if body.signature().to_string().is_empty() {
            Vec::new()
        } else {
            match body.deserialize::<zbus::wire::Structure<'_>>() {
                Ok(structure) => structure.fields().iter().map(from_variant).collect(),
                Err(error) => {
                    tracing::warn!("Cannot decode {interface}.{member}: {error}");
                    return None;
                }
            }
        };
        Some(DbusEvent {
            interface,
            member,
            args,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignalHandler {
    PropertiesChanged,
    RecoveryUpdateFailed,
    RaucUpdateComplete,
    NotifyUsb,
    NotifyUsbTest,
    RecoveryUpdate,
    DownloadProgress,
    ReportHawkbitError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PropertyHandler {
    InstallProgress,
    ReportError,
}

struct Subscription {
    handler: SignalHandler,
    /// GDBus calls the callback once per subscription that matches.
    count: usize,
}

struct State {
    subscriptions: HashMap<(String, String), Subscription>,
    properties: HashMap<(String, String), PropertyHandler>,
    /// `progress_notification`: one notification, updated in place.
    progress_notification: Notification,
    /// `usb_device_notification`.
    usb_device_notification: Notification,
    /// `error_rauc_updating`: RAUC's last error, "" when none.
    error_rauc_updating: String,
}

pub struct DbusMonitor {
    config: Arc<ConfigStore>,
    notifications: Arc<NotificationManager>,
    os_update: Arc<OsUpdateStatus>,
    host: Arc<Host>,
    usb_test_mount: PathBuf,
    state: Mutex<State>,
    connection: Mutex<Option<zbus::Connection>>,
}

/// An exception a callback raises, as `str(e)`.
type HandlerError = String;

fn arg(args: &[DValue], index: usize) -> Result<&DValue, HandlerError> {
    args.get(index)
        .ok_or_else(|| "tuple index out of range".to_string())
}

impl DbusMonitor {
    pub fn new(
        config: Arc<ConfigStore>,
        notifications: Arc<NotificationManager>,
        os_update: Arc<OsUpdateStatus>,
        host: Arc<Host>,
        usb_test_mount: PathBuf,
    ) -> DbusMonitor {
        let mut subscriptions = HashMap::new();
        for (interface, member, handler) in [
            // AsyncDBUSClient.__init__
            (
                "org.freedesktop.DBus.Properties",
                "PropertiesChanged",
                SignalHandler::PropertiesChanged,
            ),
            // DBusMonitor.init
            (
                "com.Meticulous.Handler.Updater",
                "UpdateFailed",
                SignalHandler::RecoveryUpdateFailed,
            ),
            (
                "de.pengutronix.rauc.Installer",
                "Completed",
                SignalHandler::RaucUpdateComplete,
            ),
            (
                "com.Meticulous.Handler.MassStorage",
                "NewUSB",
                SignalHandler::NotifyUsb,
            ),
            (
                "com.Meticulous.Handler.MassStorage",
                "RecoveryUpdate",
                SignalHandler::RecoveryUpdate,
            ),
            (
                "org.hawkbit.DownloadProgress",
                "ProgressUpdate",
                SignalHandler::DownloadProgress,
            ),
            (
                "org.hawkbit.DownloadProgress",
                "Error",
                SignalHandler::ReportHawkbitError,
            ),
        ] {
            subscriptions.insert(
                (interface.to_string(), member.to_string()),
                Subscription { handler, count: 1 },
            );
        }
        let mut properties = HashMap::new();
        properties.insert(
            (
                "de.pengutronix.rauc.Installer".to_string(),
                "Progress".to_string(),
            ),
            PropertyHandler::InstallProgress,
        );
        properties.insert(
            (
                "de.pengutronix.rauc.Installer".to_string(),
                "LastError".to_string(),
            ),
            PropertyHandler::ReportError,
        );
        DbusMonitor {
            config,
            notifications,
            os_update,
            host,
            usb_test_mount,
            state: Mutex::new(State {
                subscriptions,
                properties,
                progress_notification: Notification::new("").image(FLASHING_NOTIFICATION_IMAGE),
                usb_device_notification: Notification::new("")
                    .image(crate::images::USB_DEVICE_NOTIFICATION_IMAGE),
                error_rauc_updating: String::new(),
            }),
            connection: Mutex::new(None),
        }
    }

    /// `DBusMonitor.init`: connect to the system bus, add a match rule per
    /// subscription and handle the signals on their own task. Without a
    /// system bus the monitor stays idle.
    pub async fn init(self: &Arc<Self>) {
        let connection = match tokio::time::timeout(
            Duration::from_secs(10),
            zbus::Connection::system(),
        )
        .await
        {
            Ok(Ok(connection)) => connection,
            Ok(Err(error)) => {
                tracing::warn!(
                    "No D-Bus system bus, OS update and USB signals are not monitored: {error}"
                );
                return;
            }
            Err(_) => {
                tracing::warn!(
                    "Timed out connecting to the D-Bus system bus, OS update and USB signals are not monitored"
                );
                return;
            }
        };
        // Start reading before the rules are in place so nothing is missed.
        let mut stream = zbus::MessageStream::from(&connection);
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<DbusEvent>();
        let forwarder = tokio::spawn(async move {
            while let Some(message) = stream.next().await {
                let Ok(message) = message else { continue };
                if let Some(event) = DbusEvent::from_message(&message)
                    && sender.send(event).is_err()
                {
                    break;
                }
            }
        });
        let keys: Vec<(String, String)> = self.state.lock().subscriptions.keys().cloned().collect();
        for (interface, member) in &keys {
            if let Err(error) = add_match(&connection, interface, member).await {
                tracing::warn!("Cannot subscribe to {interface}.{member}: {error}");
            }
        }
        *self.connection.lock() = Some(connection);
        let monitor = Arc::clone(self);
        tokio::spawn(async move {
            while let Some(event) = receiver.recv().await {
                let handler = monitor.clone();
                // Callbacks run commands and sleep; keep them off the runtime
                // and one at a time, like the Python's event loop.
                if tokio::task::spawn_blocking(move || handler.dispatch(&event))
                    .await
                    .is_err()
                {
                    tracing::error!("Dbus monitor loop failed");
                }
            }
            forwarder.abort();
        });
    }

    /// `DBusMonitor.enableUSBTest`: in manufacturing, a new USB stick is
    /// written and read back, and a notification says whether it worked.
    pub async fn enable_usb_test(&self) {
        let last_boot_mode = self.config.get(keys::MANUFACTURING, keys::LAST_BOOT_MODE);
        if last_boot_mode != Value::String("manufacturing".into()) {
            return;
        }
        tracing::info!("subscribing to usb test signal on dbus");
        let (interface, member) = ("com.Meticulous.Handler.MassStorage", "NewUSB");
        {
            let mut state = self.state.lock();
            let entry = state
                .subscriptions
                .entry((interface.to_string(), member.to_string()))
                .or_insert(Subscription {
                    handler: SignalHandler::NotifyUsbTest,
                    count: 0,
                });
            // PARITY: the callback is replaced, but GDBus keeps both
            // subscriptions, so each NewUSB runs the USB test twice.
            entry.handler = SignalHandler::NotifyUsbTest;
            entry.count += 1;
        }
        let connection = self.connection.lock().clone();
        if let Some(connection) = connection
            && let Err(error) = add_match(&connection, interface, member).await
        {
            tracing::warn!("Cannot subscribe to {interface}.{member}: {error}");
        }
    }

    /// `handle_dbus_event`: run the callback of a signal (once per
    /// subscription), logging what it raises.
    pub fn dispatch(&self, event: &DbusEvent) {
        let subscription = self
            .state
            .lock()
            .subscriptions
            .get(&(event.interface.clone(), event.member.clone()))
            .map(|s| (s.handler, s.count));
        let Some((handler, count)) = subscription else {
            return;
        };
        for _ in 0..count {
            if let Err(error) = self.run_signal(handler, &event.args) {
                tracing::info!("handling dbus events exception: [{error}]");
                tracing::error!("{error}");
            }
        }
    }

    fn run_signal(&self, handler: SignalHandler, args: &[DValue]) -> Result<(), HandlerError> {
        match handler {
            SignalHandler::PropertiesChanged => self.property_changed(args),
            SignalHandler::RecoveryUpdateFailed => self.recovery_update_failed(args),
            SignalHandler::RaucUpdateComplete => self.rauc_update_complete(),
            SignalHandler::NotifyUsb => self.notify_usb(args),
            SignalHandler::NotifyUsbTest => self.notify_usb_test(args),
            SignalHandler::RecoveryUpdate => {
                self.os_update.mark_as_recovery_update(true);
                tracing::info!("Update in course is a recovery update");
                Ok(())
            }
            SignalHandler::DownloadProgress => self.download_progress(args),
            SignalHandler::ReportHawkbitError => self.report_hawkbit_error(args),
        }
    }

    /// `property_changed_callback`: the watched properties of a
    /// `PropertiesChanged`, as if they were signals.
    fn property_changed(&self, args: &[DValue]) -> Result<(), HandlerError> {
        let interface = arg(args, 0)?.python_str();
        let DValue::Dict(changed) = arg(args, 1)? else {
            return Err("'tuple' object has no attribute 'items'".into());
        };
        let watched: Vec<(PropertyHandler, DValue)> = {
            let state = self.state.lock();
            changed
                .iter()
                .filter_map(|(name, value)| {
                    state
                        .properties
                        .get(&(interface.clone(), name.python_str()))
                        .map(|handler| (*handler, value.clone()))
                })
                .collect()
        };
        for (handler, value) in watched {
            match handler {
                PropertyHandler::InstallProgress => self.install_progress(&value)?,
                PropertyHandler::ReportError => self.report_error(&value),
            }
        }
        Ok(())
    }

    fn add_progress_notification(&self) {
        let notification = self.state.lock().progress_notification.clone();
        self.notifications.add(notification);
    }

    /// "dismiss progress notification": the same notification, emptied.
    fn dismiss_progress_notification(&self) {
        {
            let mut state = self.state.lock();
            state.progress_notification.image = Some(String::new());
            state.progress_notification.message = String::new();
        }
        self.add_progress_notification();
    }

    fn notify(&self, message: String) {
        self.notifications.add(
            Notification::with_responses(message, &[responses::OK])
                .image(FLASHING_NOTIFICATION_IMAGE),
        );
    }

    /// `download_progress`: rauc-hawkbit-updater's download percentage.
    fn download_progress(&self, args: &[DValue]) -> Result<(), HandlerError> {
        let percentage = arg(args, 0)?;
        let rounded = match percentage {
            DValue::Int(i) => *i as f64,
            DValue::Float(f) if f.is_finite() => f.round_ties_even(),
            DValue::Float(_) => {
                return Err("cannot convert float NaN or infinity to integer".into());
            }
            DValue::Bool(b) => f64::from(u8::from(*b)),
            _ => return Err("type doesn't define __round__ method".into()),
        };
        self.os_update
            .send_status(OsStatus::Downloading, rounded, None);
        if self.os_update.is_recovery_update() {
            {
                let mut state = self.state.lock();
                state.progress_notification.message =
                    format!("Downloading update: {}%", percentage.python_str());
                state.progress_notification.image = Some(FLASHING_NOTIFICATION_IMAGE.to_string());
            }
            self.add_progress_notification();
        }
        Ok(())
    }

    /// `report_hawkbit_error`.
    fn report_hawkbit_error(&self, args: &[DValue]) -> Result<(), HandlerError> {
        let process = arg(args, 0)?;
        let error = arg(args, 1)?;
        let process = if process.as_str() == Some("EPRODEP") {
            "processing deployment"
        } else {
            "downloading"
        };
        tracing::error!("Error in {process} process: {}", error.python_str());
        Ok(())
    }

    /// `install_progress`: RAUC's `Progress` property, (percentage, message,
    /// depth).
    fn install_progress(&self, status: &DValue) -> Result<(), HandlerError> {
        let DValue::List(fields) = status else {
            return Err("cannot unpack non-iterable object".into());
        };
        let [progress, _message, _depth] = fields.as_slice() else {
            return Err(format!(
                "not enough values to unpack (expected 3, got {})",
                fields.len()
            ));
        };
        {
            let mut state = self.state.lock();
            state.progress_notification.message =
                format!("Updating OS:\n {}%", progress.python_str());
            state.progress_notification.image = Some(FLASHING_NOTIFICATION_IMAGE.to_string());
        }
        let value = progress.as_number().unwrap_or(0.0);
        self.os_update
            .send_status(OsStatus::Installing, value, None);
        if self.os_update.is_recovery_update() {
            self.add_progress_notification();
        }
        Ok(())
    }

    /// `report_error`: RAUC's `LastError` property.
    fn report_error(&self, status: &DValue) {
        let status = status.python_str();
        self.state.lock().error_rauc_updating = status.clone();
        if status.is_empty() {
            return;
        }
        self.os_update
            .send_status(OsStatus::Failed, 0.0, Some(&status));
        if self.os_update.is_recovery_update() {
            self.dismiss_progress_notification();
            self.notify(format!("There was an error updating the OS:\n {status}"));
        }
        self.os_update.mark_as_recovery_update(false);
        self.clean_possible_updater();
    }

    /// `rauc_update_complete`: RAUC's `Completed` signal.
    fn rauc_update_complete(&self) -> Result<(), HandlerError> {
        self.os_update.send_status(OsStatus::Complete, 100.0, None);
        let error = self.state.lock().error_rauc_updating.clone();
        let message = if error.is_empty() {
            "OS updated. Remove USB and reboot your machine".to_string()
        } else {
            tracing::info!("error is [{error}]");
            format!("Failed OS updated no need to reboot your machine\n Error: {error}")
        };
        if self.os_update.is_recovery_update() {
            self.dismiss_progress_notification();
            self.notify(message);
        }
        self.os_update.mark_as_recovery_update(false);
        self.clean_possible_updater();
        Ok(())
    }

    /// Unmount and remove where the USB handler mounted the update.
    fn clean_possible_updater(&self) {
        for command in [
            format!("umount {POSSIBLE_UPDATER}"),
            format!("rm -r {POSSIBLE_UPDATER}"),
        ] {
            match self.host.run_shell(&command) {
                Ok(output) => tracing::warn!(
                    "CompletedProcess(args='{command}', returncode={}, stdout={:?}, stderr={:?})",
                    output.returncode(),
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ),
                Err(error) => tracing::warn!("{command}: {error}"),
            }
        }
    }

    /// `notify_usb`.
    fn notify_usb(&self, args: &[DValue]) -> Result<(), HandlerError> {
        tracing::info!(
            "received signal NEW USB with parameters: [{}]",
            DValue::List(args.to_vec()).python_repr()
        );
        let usb_path = arg(args, 0)?;
        tracing::info!("USB PATH RECEIVED: {}", usb_path.python_str());
        Ok(())
    }

    /// `notify_usb_test`: mount the stick, write a file named after the
    /// machine, read it back after two seconds and say so.
    fn notify_usb_test(&self, args: &[DValue]) -> Result<(), HandlerError> {
        let usb_path = arg(args, 0)?.python_str();
        tracing::info!("USB Device connected, {usb_path}");
        self.state.lock().usb_device_notification.message = "USB valid".into();
        let mount = self.usb_test_mount.to_string_lossy().into_owned();
        std::fs::create_dir_all(&self.usb_test_mount).map_err(|e| e.to_string())?;
        match self.host.run(&["mount", &usb_path, &mount]) {
            Ok(output) if output.success() => {
                let serial = self.config.get(keys::SYSTEM, keys::MACHINE_SERIAL_NUMBER);
                let serial = if serial.is_null() {
                    "unknownSN".to_string()
                } else {
                    crate::ota::python_str(&serial)
                };
                let machine_name = format!("{}-{serial}", generate_hostname(&self.config));
                let test_path = self.usb_test_mount.join(&machine_name);
                let tested = std::fs::write(&test_path, &machine_name).and_then(|()| {
                    std::thread::sleep(Duration::from_secs(2));
                    std::fs::read_to_string(&test_path)
                });
                match tested {
                    Ok(read) if read == machine_name => {
                        let notification = self.state.lock().usb_device_notification.clone();
                        self.notifications.add(notification);
                    }
                    Ok(_) => {}
                    Err(error) => tracing::warn!("error testing '{usb_path}' device: {error}"),
                }
            }
            Ok(output) => tracing::warn!(
                "Cannot mount '{usb_path}' device: {}",
                crate::host::describe_failure(&["mount", &usb_path, &mount], &output)
            ),
            Err(error) => tracing::warn!("error testing '{usb_path}' device: {error}"),
        }
        match self.host.run(&["umount", &mount]) {
            Ok(output) if output.success() => Ok(()),
            Ok(output) => {
                tracing::warn!(
                    "error unmounting '{mount}': {}",
                    crate::host::describe_failure(&["umount", &mount], &output)
                );
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }

    /// `recovery_update_failed`: meticulous-machine's USB updater gave up.
    fn recovery_update_failed(&self, args: &[DValue]) -> Result<(), HandlerError> {
        let reason = arg(args, 0)?;
        let shown = if reason.as_str() == Some("unknown") {
            "unknown error, possible USB disconnection".to_string()
        } else {
            reason.python_str()
        };
        let message = format!("Recovery Update Failed:\n {shown}");
        {
            let mut state = self.state.lock();
            state.progress_notification.image = Some(String::new());
            state.progress_notification.message = String::new();
        }
        self.os_update
            .send_status(OsStatus::Failed, 0.0, Some(&reason.python_str()));
        self.add_progress_notification();
        self.notify(message.clone());
        self.os_update.mark_as_recovery_update(false);
        tracing::info!("RECOVERY UPDATE FAILED: {message}");
        Ok(())
    }
}

/// `AddMatch` for every signal with this interface and member, from any
/// sender on any path (`signal_subscribe(None, interface, member, None, ...)`).
async fn add_match(
    connection: &zbus::Connection,
    interface: &str,
    member: &str,
) -> zbus::Result<()> {
    let rule = format!("type='signal',interface='{interface}',member='{member}'");
    connection
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "AddMatch",
            &(rule,),
        )
        .await
        .map(|_| ())
}

/// `HostnameManager.generateHostname` (the hostname service owns it; same
/// formula).
fn generate_hostname(config: &ConfigStore) -> String {
    let ident = config.get(keys::SYSTEM, keys::DEVICE_IDENTIFIER);
    let camel = match ident.as_array().map(Vec::as_slice) {
        Some([first, second]) => {
            let first = crate::ota::python_str(first);
            let mut chars = first.chars();
            let capitalized: String = match chars.next() {
                Some(c) => c
                    .to_uppercase()
                    .chain(chars.flat_map(char::to_lowercase))
                    .collect(),
                None => String::new(),
            };
            format!("{capitalized}{}", crate::ota::python_str(second))
        }
        _ => "Espresso".to_string(),
    };
    format!(
        "meticulous{camel}-{}",
        crate::ota::python_str(&config.get(keys::SYSTEM, keys::MACHINE_SERIAL_NUMBER))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::fake::FakeRunner;
    use crate::testing::app;
    use serde_json::json;

    fn int(i: i64) -> DValue {
        DValue::Int(i)
    }

    fn text(s: &str) -> DValue {
        DValue::Str(s.to_string())
    }

    fn rauc_property(name: &str, value: DValue) -> DbusEvent {
        DbusEvent::new(
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            vec![
                text("de.pengutronix.rauc.Installer"),
                DValue::Dict(vec![(text(name), value)]),
                DValue::List(vec![]),
            ],
        )
    }

    fn messages(app: &crate::app::App) -> Vec<(String, Option<String>)> {
        app.notifications
            .all()
            .into_iter()
            .map(|n| (n.message, n.image))
            .collect()
    }

    /// A Hawkbit download outside a recovery update only moves the status.
    #[tokio::test]
    async fn download_progress_updates_the_os_status() {
        let (_dir, app) = app();
        app.dbus.dispatch(&DbusEvent::new(
            "org.hawkbit.DownloadProgress",
            "ProgressUpdate",
            vec![DValue::Float(42.5)],
        ));
        assert_eq!(
            app.os_update.to_json(),
            json!({"progress": 42, "status": "DOWNLOADING", "info": ""})
        );
        assert!(app.notifications.all().is_empty());
    }

    /// A USB recovery update shows its progress in one notification and
    /// ends with a reboot notice; the updater mount is cleaned up.
    #[tokio::test]
    async fn recovery_update_shows_progress_and_completion() {
        let (_dir, app) = app();
        let runner = Arc::new(FakeRunner::default());
        app.host.replace(runner.clone());
        app.dbus.dispatch(&DbusEvent::new(
            "com.Meticulous.Handler.MassStorage",
            "RecoveryUpdate",
            vec![],
        ));
        assert!(app.os_update.is_recovery_update());

        app.dbus.dispatch(&DbusEvent::new(
            "org.hawkbit.DownloadProgress",
            "ProgressUpdate",
            vec![int(10)],
        ));
        app.dbus.dispatch(&rauc_property(
            "Progress",
            DValue::List(vec![int(55), text("Installing"), int(1)]),
        ));
        let listed = messages(&app);
        assert_eq!(listed.len(), 1, "one progress notification, updated");
        assert_eq!(listed[0].0, "Updating OS:\n 55%");
        assert_eq!(
            app.os_update.to_json(),
            json!({"progress": 55, "status": "INSTALLING", "info": ""})
        );

        app.dbus.dispatch(&DbusEvent::new(
            "de.pengutronix.rauc.Installer",
            "Completed",
            vec![int(0)],
        ));
        let listed = messages(&app);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0], (String::new(), Some(String::new())));
        assert_eq!(
            listed[1].0,
            "OS updated. Remove USB and reboot your machine"
        );
        assert_eq!(
            listed[1].1.as_deref(),
            Some(crate::images::FLASHING_NOTIFICATION_IMAGE)
        );
        assert!(!app.os_update.is_recovery_update());
        assert_eq!(
            app.os_update.to_json(),
            json!({"progress": 100, "status": "COMPLETE", "info": ""})
        );
        assert_eq!(
            runner.argvs(),
            vec![
                vec!["/bin/sh", "-c", "umount /tmp/possible_updater"],
                vec!["/bin/sh", "-c", "rm -r /tmp/possible_updater"],
            ]
        );
    }

    /// RAUC's LastError fails the update and changes the completion message.
    #[tokio::test]
    async fn rauc_error_fails_the_update() {
        let (_dir, app) = app();
        app.host.replace(Arc::new(FakeRunner::default()));
        app.os_update.mark_as_recovery_update(true);
        app.dbus.dispatch(&rauc_property("LastError", text("")));
        assert!(app.notifications.all().is_empty());

        app.dbus
            .dispatch(&rauc_property("LastError", text("bad bundle")));
        assert_eq!(
            app.os_update.to_json(),
            json!({"progress": 0, "status": "FAILED", "info": ""})
        );
        let listed = messages(&app);
        assert_eq!(
            listed[1].0,
            "There was an error updating the OS:\n bad bundle"
        );
        assert!(!app.os_update.is_recovery_update());

        app.os_update.mark_as_recovery_update(true);
        app.dbus.dispatch(&DbusEvent::new(
            "de.pengutronix.rauc.Installer",
            "Completed",
            vec![int(1)],
        ));
        let listed = messages(&app);
        assert_eq!(
            listed.last().map(|m| m.0.clone()).as_deref(),
            Some("Failed OS updated no need to reboot your machine\n Error: bad bundle")
        );
    }

    /// The USB updater's failure always notifies, unknown reasons explained.
    #[tokio::test]
    async fn recovery_update_failure_notifies() {
        let (_dir, app) = app();
        app.dbus.dispatch(&DbusEvent::new(
            "com.Meticulous.Handler.Updater",
            "UpdateFailed",
            vec![text("unknown")],
        ));
        let listed = messages(&app);
        assert_eq!(
            listed[1].0,
            "Recovery Update Failed:\n unknown error, possible USB disconnection"
        );
        assert_eq!(
            app.os_update.to_json(),
            json!({"progress": 0, "status": "FAILED", "info": ""})
        );
    }

    /// A malformed signal is logged, not fatal; Hawkbit errors only log.
    #[tokio::test]
    async fn malformed_signals_are_logged() {
        let (_dir, app) = app();
        app.dbus.dispatch(&DbusEvent::new(
            "org.hawkbit.DownloadProgress",
            "ProgressUpdate",
            vec![],
        ));
        app.dbus.dispatch(&DbusEvent::new(
            "org.hawkbit.DownloadProgress",
            "Error",
            vec![text("EPRODEP"), text("boom")],
        ));
        app.dbus
            .dispatch(&DbusEvent::new("org.example", "Unrelated", vec![int(1)]));
        assert_eq!(
            app.os_update.to_json(),
            json!({"progress": 0, "status": "IDLE", "info": ""})
        );
    }

    /// enableUSBTest only in manufacturing; then each NewUSB runs the test
    /// twice (PARITY with the doubled GDBus subscription).
    #[tokio::test]
    async fn usb_test_in_manufacturing() {
        let (_dir, app) = app();
        let runner = Arc::new(FakeRunner::default());
        app.host.replace(runner.clone());
        app.dbus.enable_usb_test().await;
        app.dbus.dispatch(&DbusEvent::new(
            "com.Meticulous.Handler.MassStorage",
            "NewUSB",
            vec![text("/dev/sda1")],
        ));
        assert!(runner.calls.lock().is_empty());

        app.config.set(
            keys::MANUFACTURING,
            keys::LAST_BOOT_MODE,
            json!("manufacturing"),
        );
        app.config.set(
            keys::SYSTEM,
            keys::DEVICE_IDENTIFIER,
            json!(["aromatic", "bean"]),
        );
        app.config
            .set(keys::SYSTEM, keys::MACHINE_SERIAL_NUMBER, json!("1234"));
        app.dbus.enable_usb_test().await;
        let mount = app.env.paths.system.usb_test_mount.clone();
        app.dbus.dispatch(&DbusEvent::new(
            "com.Meticulous.Handler.MassStorage",
            "NewUSB",
            vec![text("/dev/sda1")],
        ));
        let mount_arg = mount.to_string_lossy().into_owned();
        assert_eq!(
            runner.argvs(),
            vec![
                vec![
                    "mount".to_string(),
                    "/dev/sda1".to_string(),
                    mount_arg.clone()
                ],
                vec!["umount".to_string(), mount_arg.clone()],
                vec![
                    "mount".to_string(),
                    "/dev/sda1".to_string(),
                    mount_arg.clone()
                ],
                vec!["umount".to_string(), mount_arg],
            ]
        );
        assert_eq!(
            std::fs::read_to_string(mount.join("meticulousAromaticbean-1234-1234")).expect("file"),
            "meticulousAromaticbean-1234-1234"
        );
        let listed = messages(&app);
        assert_eq!(listed.len(), 1, "the same notification, updated");
        assert_eq!(listed[0].0, "USB valid");
    }

    #[test]
    fn hostname_like_hostname_manager() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = ConfigStore::open(dir.path().join("c.yml"), crate::config::defaults());
        assert_eq!(generate_hostname(&config), "meticulousEspresso-None");
        config.set(
            keys::SYSTEM,
            keys::DEVICE_IDENTIFIER,
            json!(["fullBodied", "Bean"]),
        );
        config.set(keys::SYSTEM, keys::MACHINE_SERIAL_NUMBER, json!("77"));
        assert_eq!(generate_hostname(&config), "meticulousFullbodiedBean-77");
    }
}
