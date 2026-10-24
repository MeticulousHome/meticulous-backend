//! `usb.py` and `PTN5150.py`: the USB-C port controller (NXP PTN5150H on
//! I2C bus 1, address 0x3D) that decides whether the port is a host or a
//! device. Registers are read and written with `i2cget`/`i2cset`, as the
//! Python does.

use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::Value;

use crate::config::{ConfigStore, keys};
use crate::host::Host;

/// `USB_MODES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbMode {
    /// Client mode with network OTG.
    Client,
    /// OTG / host mode without networking.
    Host,
    DualRole,
}

impl UsbMode {
    pub fn as_str(self) -> &'static str {
        match self {
            UsbMode::Client => "client",
            UsbMode::Host => "host",
            UsbMode::DualRole => "dual_role",
        }
    }
}

/// `USB_MODES(value)`: the `ValueError` text is what the settings API puts
/// after "Failed to set the USB mode: ".
pub fn parse_usb_mode(value: &Value) -> Result<UsbMode, String> {
    match value.as_str() {
        Some("client") => Ok(UsbMode::Client),
        Some("host") => Ok(UsbMode::Host),
        Some("dual_role") => Ok(UsbMode::DualRole),
        _ => Err(format!("{} is not a valid USB_MODES", python_repr(value))),
    }
}

/// `repr()` of a settings value, for error texts.
pub fn python_repr(value: &Value) -> String {
    match value {
        Value::String(s) => {
            if s.contains('\'') && !s.contains('"') {
                format!("\"{s}\"")
            } else {
                format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
            }
        }
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(python_repr).collect::<Vec<_>>().join(", ")
        ),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!(
                    "{}: {}",
                    python_repr(&Value::String(k.clone())),
                    python_repr(v)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => crate::ota::python_str(other),
    }
}

/// Bits [2:1] of the control register (0x02).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortState {
    Ufp = 0,
    Dfp = 1,
    Drp = 2,
}

/// Bits [4:3] of the control register (0x02): Rp current in DFP mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpSelection {
    X80uA = 0,
    X180uA = 1,
    X330uA = 2,
}

/// `PTN5150H`.
pub struct Ptn5150h {
    host: Arc<Host>,
    i2cbus: u8,
    address: u8,
}

const CONTROL_REGISTER: u8 = 0x02;

impl Ptn5150h {
    pub fn new(host: Arc<Host>) -> Ptn5150h {
        Ptn5150h {
            host,
            i2cbus: 1,
            address: 0x3D,
        }
    }

    /// `read_register`: `None` where the Python returns -1.
    pub fn read_register(&self, register: u8) -> Option<u8> {
        let bus = self.i2cbus.to_string();
        let address = format!("0x{:02x}", self.address);
        let register_arg = format!("0x{register:02x}");
        let argv = ["i2cget", "-f", "-y", &bus, &address, &register_arg];
        match self.host.run(&argv) {
            Ok(output) if output.success() => {
                let text = output.stdout_text();
                let text = text.trim();
                let digits = text
                    .strip_prefix("0x")
                    .or_else(|| text.strip_prefix("0X"))
                    .unwrap_or(text);
                u8::from_str_radix(digits, 16).ok()
            }
            Ok(output) => {
                tracing::warn!(
                    "Error reading register {register}: {}",
                    crate::host::describe_failure(&argv, &output)
                );
                None
            }
            Err(error) => {
                tracing::warn!("Error reading register {register}: {error}");
                None
            }
        }
    }

    /// `write_register`.
    pub fn write_register(&self, register: u8, value: u8) {
        let bus = self.i2cbus.to_string();
        let address = format!("0x{:02x}", self.address);
        let register_arg = format!("0x{register:02x}");
        let value_arg = format!("0x{value:02x}");
        let argv = [
            "i2cset",
            "-f",
            "-y",
            &bus,
            &address,
            &register_arg,
            &value_arg,
        ];
        match self.host.run(&argv) {
            Ok(output) if output.success() => {}
            Ok(output) => tracing::warn!(
                "Error writing register {register}: {}",
                crate::host::describe_failure(&argv, &output)
            ),
            Err(error) => tracing::warn!("Error writing register {register}: {error}"),
        }
    }

    /// `set_port_state`.
    pub fn set_port_state(&self, state: PortState) {
        if let Some(value) = self.read_register(CONTROL_REGISTER) {
            self.write_register(
                CONTROL_REGISTER,
                (value & 0xF9) | ((state as u8 & 0x03) << 1),
            );
        }
    }

    /// `set_rp_selection`.
    pub fn set_rp_selection(&self, selection: RpSelection) {
        if let Some(value) = self.read_register(CONTROL_REGISTER) {
            self.write_register(
                CONTROL_REGISTER,
                (value & 0xE7) | ((selection as u8 & 0x03) << 3),
            );
        }
    }
}

pub struct UsbManager {
    config: Arc<ConfigStore>,
    host: Arc<Host>,
    fake_hardware: bool,
    controller: Mutex<Option<Ptn5150h>>,
}

impl UsbManager {
    pub fn new(config: Arc<ConfigStore>, host: Arc<Host>, fake_hardware: bool) -> UsbManager {
        UsbManager {
            config,
            host,
            fake_hardware,
            controller: Mutex::new(None),
        }
    }

    /// `USBManager.init`: apply the configured mode at boot.
    pub fn init(&self, emulated: bool) {
        if emulated {
            return;
        }
        // The boot harness replaces PTN5150H with a mock; constructing it
        // touches nothing either way, and setUSBMode never reaches it.
        if !self.fake_hardware {
            *self.controller.lock() = Some(Ptn5150h::new(self.host.clone()));
        }
        tracing::info!("USB PD Controller initialized");
        let configured = self.config.get(keys::USER, keys::USB_MODE);
        match parse_usb_mode(&configured) {
            Ok(mode) => self.set_usb_mode(mode, emulated),
            Err(error) => {
                // The Python lets this ValueError stop the boot; start the
                // port as a client like its RuntimeError branch instead.
                tracing::error!("error initializing USB Manager: f{error}, starting USB as client");
                self.set_usb_mode(UsbMode::Client, emulated);
            }
        }
    }

    /// `USBManager.setUSBMode`, for boot and the settings API.
    pub fn set_usb_mode(&self, mode: UsbMode, emulated: bool) {
        // PARITY: switching the port is disabled in the Python; the code
        // below is what runs once it is enabled again.
        tracing::warn!("Setting USB mode is temporarely disabled");
        let enabled = false;
        if !enabled {
            return;
        }
        if emulated {
            tracing::info!("Emulated machine, skipping USB mode setup");
            return;
        }
        let controller = self.controller.lock();
        let Some(ptn) = controller.as_ref() else {
            return;
        };
        match mode {
            UsbMode::Client => {
                tracing::info!("Setting USB mode to CLIENT");
                ptn.set_port_state(PortState::Ufp);
            }
            UsbMode::Host => {
                tracing::info!("Setting USB mode to HOST");
                ptn.set_port_state(PortState::Dfp);
            }
            UsbMode::DualRole => {
                tracing::info!("Setting USB mode to DUAL");
                ptn.set_port_state(PortState::Drp);
            }
        }
        ptn.set_rp_selection(RpSelection::X330uA);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::CommandOutput;
    use crate::host::fake::FakeRunner;
    use serde_json::json;

    /// tests/test_api_settings.py::test_post_valid_usb_mode_is_saved and
    /// ::test_post_unknown_usb_mode_returns_400: the mode check and its text.
    #[test]
    fn usb_mode_validation() {
        assert_eq!(parse_usb_mode(&json!("client")), Ok(UsbMode::Client));
        assert_eq!(parse_usb_mode(&json!("host")), Ok(UsbMode::Host));
        assert_eq!(parse_usb_mode(&json!("dual_role")), Ok(UsbMode::DualRole));
        assert_eq!(
            parse_usb_mode(&json!("otg")),
            Err("'otg' is not a valid USB_MODES".to_string())
        );
        assert_eq!(
            parse_usb_mode(&json!(3)),
            Err("3 is not a valid USB_MODES".to_string())
        );
    }

    /// setUSBMode is disabled: it never reaches the controller.
    #[test]
    fn setting_the_mode_touches_no_register() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = Arc::new(ConfigStore::open(
            dir.path().join("config.yml"),
            crate::config::defaults(),
        ));
        let host = Arc::new(Host::default());
        let runner = Arc::new(FakeRunner::default());
        host.replace(runner.clone());
        let usb = UsbManager::new(config, host, false);
        usb.init(false);
        usb.set_usb_mode(UsbMode::Client, false);
        assert!(runner.calls.lock().is_empty());
    }

    /// PTN5150H register access: bits [2:1] and [4:3] of the control register.
    #[test]
    fn controller_register_writes() {
        let host = Arc::new(Host::default());
        let runner = Arc::new(FakeRunner::default());
        host.replace(runner.clone());
        runner.answer(
            "i2cget",
            CommandOutput {
                status: Some(0),
                stdout: b"0x1f\n".to_vec(),
                stderr: Vec::new(),
            },
        );
        let ptn = Ptn5150h::new(host);
        ptn.set_port_state(PortState::Dfp);
        ptn.set_rp_selection(RpSelection::X330uA);
        assert_eq!(
            runner.argvs(),
            vec![
                vec!["i2cget", "-f", "-y", "1", "0x3d", "0x02"],
                vec!["i2cset", "-f", "-y", "1", "0x3d", "0x02", "0x1b"],
                vec!["i2cget", "-f", "-y", "1", "0x3d", "0x02"],
                vec!["i2cset", "-f", "-y", "1", "0x3d", "0x02", "0x17"],
            ]
        );
    }
}
