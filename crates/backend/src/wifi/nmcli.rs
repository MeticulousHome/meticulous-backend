//! The `nmcli` Python package (python-nmcli 1.7): the NetworkManager calls
//! `wifi.py` makes through it, with its command lines, its parsing of
//! nmcli's text output and its exceptions, whose messages reach clients.
//!
//! `nmcli.disable_use_sudo()` and `nmcli.set_lang("C.UTF-8")` are in effect:
//! commands run as `nmcli ...` with `LANG=C.UTF-8`.

use std::sync::Arc;
use std::sync::LazyLock;

use regex::Regex;

use crate::py::{CommandRunner, PyError, PyResult, RunError, run_command};
use crate::wifi::types::{Connection, Device, DeviceDetails, ScanEntry};

/// What `wifi.py` calls on `nmcli.device` and `nmcli.connection`.
pub trait Nmcli: Send + Sync {
    /// `nmcli.device()`.
    fn device_status(&self) -> PyResult<Vec<Device>>;
    /// `nmcli.device.show(ifname)`.
    fn device_show(&self, ifname: &str) -> PyResult<DeviceDetails>;
    /// `nmcli.device.show_all()`.
    fn device_show_all(&self) -> PyResult<Vec<DeviceDetails>>;
    /// `nmcli.device.wifi(rescan=...)`.
    fn device_wifi(&self, rescan: Option<bool>) -> PyResult<Vec<ScanEntry>>;
    /// `nmcli.device.wifi_connect(ssid, password)`.
    fn device_wifi_connect(&self, ssid: &str, password: Option<&str>) -> PyResult<()>;
    /// `nmcli.connection()`.
    fn connections(&self) -> PyResult<Vec<Connection>>;
    /// `nmcli.connection.up(name, wait=...)`.
    fn connection_up(&self, name: &str, wait: Option<u32>) -> PyResult<()>;
    /// `nmcli.connection.down(name)`.
    fn connection_down(&self, name: &str) -> PyResult<()>;
    /// `nmcli.connection.delete(name)`.
    fn connection_delete(&self, name: &str) -> PyResult<()>;
    /// `nmcli.connection.modify(name, options)`.
    fn connection_modify(&self, name: &str, options: &[(&str, &str)]) -> PyResult<()>;
}

/// The real thing: the `nmcli` binary.
pub struct NmcliCli {
    runner: Option<Arc<dyn CommandRunner>>,
}

impl NmcliCli {
    pub fn new() -> NmcliCli {
        NmcliCli { runner: None }
    }

    /// Run through another runner (tests): `LANG` is not passed then.
    #[cfg(test)]
    pub fn with_runner(runner: Arc<dyn CommandRunner>) -> NmcliCli {
        NmcliCli {
            runner: Some(runner),
        }
    }

    /// `SystemCommand.nmcli(parameters)`.
    fn nmcli(&self, parameters: &[&str]) -> PyResult<String> {
        let mut argv = vec!["nmcli".to_string()];
        argv.extend(parameters.iter().map(|p| p.to_string()));
        let result = match &self.runner {
            Some(runner) => runner.run(&argv, None),
            None => run_command(&argv, None, &[("LANG", "C.UTF-8")]),
        };
        let done = result.map_err(|e: RunError| e.to_py())?;
        if done.returncode == 0 {
            return Ok(done.stdout);
        }
        Err(exception_for(done.returncode, &done.stderr))
    }
}

impl Default for NmcliCli {
    fn default() -> Self {
        NmcliCli::new()
    }
}

/// The package's exception for an nmcli exit code.
fn exception_for(returncode: i32, stderr: &str) -> PyError {
    let (kind, message) = match returncode {
        2 => (
            "InvalidUserInputException",
            "Invalid user input, wrong nmcli invocation".to_string(),
        ),
        3 => ("TimeoutExpiredException", "Timeout expired".to_string()),
        4 => (
            "ConnectionActivateFailedException",
            "Connection activation failed".to_string(),
        ),
        5 => (
            "ConnectionDeactivateFailedException",
            "Connection deactivation failed".to_string(),
        ),
        6 => (
            "DisconnectDeviceFailedException",
            "Disconnecting device failed".to_string(),
        ),
        7 => (
            "ConnectionDeleteFailedException",
            "Connection deletion failed".to_string(),
        ),
        8 => (
            "NetworkManagerNotRunningException",
            "NetworkManager is not running".to_string(),
        ),
        10 => (
            "NotExistException",
            "Connection, device, or access point does not exist".to_string(),
        ),
        // `stderr.find(...) > 0`: a match at offset 0 does not count.
        1 if stderr.find("Scanning not allowed").is_some_and(|i| i > 0) => {
            ("ScanningNotAllowedException", stderr.to_string())
        }
        rc => (
            "UnspecifiedException",
            format!("Unknown or unspecified error [code:{rc}, detail:{stderr}]"),
        ),
    };
    PyError::new(kind, message)
}

fn wait_option(wait: Option<u32>) -> Vec<String> {
    match wait {
        Some(w) => vec!["--wait".into(), w.to_string()],
        None => Vec::new(),
    }
}

impl Nmcli for NmcliCli {
    fn device_status(&self) -> PyResult<Vec<Device>> {
        let out = self.nmcli(&["device", "status"])?;
        out.split('\n')
            .skip(1)
            .filter(|row| !row.is_empty())
            .map(parse_device)
            .collect()
    }

    fn device_show(&self, ifname: &str) -> PyResult<DeviceDetails> {
        let out = self.nmcli(&["device", "show", ifname])?;
        Ok(parse_details(&out))
    }

    fn device_show_all(&self) -> PyResult<Vec<DeviceDetails>> {
        let out = self.nmcli(&["device", "show"])?;
        Ok(parse_details_all(&out))
    }

    fn device_wifi(&self, rescan: Option<bool>) -> PyResult<Vec<ScanEntry>> {
        let mut cmd = vec![
            "-t",
            "-f",
            "IN-USE,SSID,BSSID,MODE,CHAN,FREQ,RATE,SIGNAL,SECURITY",
            "device",
            "wifi",
            "list",
        ];
        if let Some(rescan) = rescan {
            cmd.extend(["--rescan", if rescan { "yes" } else { "no" }]);
        }
        let out = self.nmcli(&cmd)?;
        out.split('\n')
            .filter(|row| !row.is_empty())
            .map(parse_scan_entry)
            .collect()
    }

    fn device_wifi_connect(&self, ssid: &str, password: Option<&str>) -> PyResult<()> {
        // PARITY: python-nmcli always passes `password <password>`; with no
        // password (an open network) `subprocess.run` rejects the None.
        let Some(password) = password else {
            return Err(PyError::type_error(
                "expected str, bytes or os.PathLike object, not NoneType",
            ));
        };
        let out = self.nmcli(&["device", "wifi", "connect", ssid, "password", password])?;
        if out.contains("Connection activation failed:") {
            return Err(PyError::new(
                "ConnectionActivateFailedException",
                "Connection activation failed",
            ));
        }
        Ok(())
    }

    fn connections(&self) -> PyResult<Vec<Connection>> {
        let out = self.nmcli(&["connection"])?;
        out.split('\n')
            .skip(1)
            .filter(|row| !row.is_empty())
            .map(parse_connection)
            .collect()
    }

    fn connection_up(&self, name: &str, wait: Option<u32>) -> PyResult<()> {
        let mut cmd = wait_option(wait);
        cmd.extend(["connection".into(), "up".into(), name.to_string()]);
        let cmd: Vec<&str> = cmd.iter().map(String::as_str).collect();
        self.nmcli(&cmd).map(|_| ())
    }

    fn connection_down(&self, name: &str) -> PyResult<()> {
        self.nmcli(&["connection", "down", name]).map(|_| ())
    }

    fn connection_delete(&self, name: &str) -> PyResult<()> {
        self.nmcli(&["connection", "delete", name]).map(|_| ())
    }

    fn connection_modify(&self, name: &str, options: &[(&str, &str)]) -> PyResult<()> {
        let mut cmd = vec!["connection", "modify", name];
        for (key, value) in options {
            cmd.push(key);
            cmd.push(value);
        }
        self.nmcli(&cmd).map(|_| ())
    }
}

fn parse_failed(text: &str) -> PyError {
    PyError::value_error(format!("Parse failed [{text}]"))
}

static DEVICE_ROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\S*)\s+(\S*)\s+(\S*)\s+([\S\s]+)\s*$").expect("static regex"));
static CONNECTION_ROW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([\S\s]+)\s{2}(\S+)\s{2}(\S+)\s+(\S+)\s*").expect("static regex")
});
static DETAIL_ROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\S+):\s*([\S\s]+)\s*").expect("static regex"));
static SCAN_ROW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(\*|\s)\x{FFFF}(.*)\x{FFFF}(.*)\x{FFFF}(.*)\x{FFFF}(\d+)\x{FFFF}(\d+)\sMHz\x{FFFF}(\d+)\s(?:Mb|Mbit)/s\x{FFFF}(\d+)\x{FFFF}(.*)$",
    )
    .expect("static regex")
});

/// `Device.parse(row)`.
pub fn parse_device(text: &str) -> PyResult<Device> {
    let caps = DEVICE_ROW
        .captures(text)
        .ok_or_else(|| parse_failed(text))?;
    let conn = caps[4].trim();
    Ok(Device {
        device: caps[1].to_string(),
        device_type: caps[2].to_string(),
        state: caps[3].to_string(),
        connection: (conn != "--").then(|| conn.to_string()),
    })
}

/// `Connection.parse(row)`.
pub fn parse_connection(text: &str) -> PyResult<Connection> {
    let caps = CONNECTION_ROW
        .captures(text)
        .ok_or_else(|| parse_failed(text))?;
    Ok(Connection {
        name: caps[1].trim().to_string(),
        uuid: caps[2].to_string(),
        conn_type: caps[3].to_string(),
        device: caps[4].to_string(),
    })
}

fn detail_value(value: &str) -> Option<String> {
    (value != "--" && value != "\"\"").then(|| value.to_string())
}

/// `DeviceControl.show`'s parsing.
pub fn parse_details(text: &str) -> DeviceDetails {
    let mut details = DeviceDetails::default();
    for row in text.split('\n') {
        if let Some(caps) = DETAIL_ROW.captures(row) {
            details.insert(&caps[1], detail_value(&caps[2]));
        }
    }
    details
}

/// `DeviceControl.show_all`'s parsing: a new device at each `GENERAL.DEVICE`.
pub fn parse_details_all(text: &str) -> Vec<DeviceDetails> {
    let mut all: Vec<DeviceDetails> = Vec::new();
    let mut current: Option<usize> = None;
    let mut orphan = DeviceDetails::default();
    for row in text.split('\n') {
        let Some(caps) = DETAIL_ROW.captures(row) else {
            continue;
        };
        if &caps[1] == "GENERAL.DEVICE" {
            all.push(DeviceDetails::default());
            current = Some(all.len() - 1);
        }
        let target = match current {
            Some(i) => &mut all[i],
            // Rows before the first device go to a dict nobody keeps.
            None => &mut orphan,
        };
        target.insert(&caps[1], detail_value(&caps[2]));
    }
    all
}

/// `DeviceWifi.parse(row)`: terse output, `\:` escapes a colon.
pub fn parse_scan_entry(text: &str) -> PyResult<ScanEntry> {
    let t = text
        .replace("\\:", "\u{FFFE}")
        .replace(':', "\u{FFFF}")
        .replace('\u{FFFE}', ":");
    let caps = SCAN_ROW.captures(&t).ok_or_else(|| parse_failed(text))?;
    let number = |i: usize| -> PyResult<i64> {
        caps[i].parse::<i64>().map_err(|_| {
            PyError::value_error(format!(
                "invalid literal for int() with base 10: '{}'",
                &caps[i]
            ))
        })
    };
    Ok(ScanEntry {
        in_use: &caps[1] == "*",
        ssid: caps[2].to_string(),
        bssid: caps[3].to_string(),
        mode: caps[4].to_string(),
        chan: number(5)?,
        freq: number(6)?,
        rate: number(7)?,
        signal: number(8)?,
        security: caps[9].trim_end().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_rows_parse_like_python_nmcli() {
        let device =
            parse_device("wlan0          wifi      connected     Home Net  ").expect("row");
        assert_eq!(device.device, "wlan0");
        assert_eq!(device.device_type, "wifi");
        assert_eq!(device.state, "connected");
        assert_eq!(device.connection.as_deref(), Some("Home Net"));
        let idle = parse_device("p2p-dev-wlan0  wifi-p2p  disconnected  --").expect("row");
        assert_eq!(idle.connection, None);
    }

    #[test]
    fn connection_rows_parse_like_python_nmcli() {
        let row = "My Home  0f6b8c1e-1111-2222-3333-444455556666  wifi      wlan0  ";
        let c = parse_connection(row).expect("row");
        assert_eq!(c.name, "My Home");
        assert_eq!(c.uuid, "0f6b8c1e-1111-2222-3333-444455556666");
        assert_eq!(c.conn_type, "wifi");
        assert_eq!(c.device, "wlan0");
    }

    #[test]
    fn scan_rows_parse_like_python_nmcli() {
        let e = parse_scan_entry(
            "*:My\\:Net:AA\\:BB\\:CC\\:DD\\:EE\\:FF:Infra:6:2437 MHz:130 Mbit/s:70:WPA2 ",
        )
        .expect("row");
        assert!(e.in_use);
        assert_eq!(e.ssid, "My:Net");
        assert_eq!(e.bssid, "AA:BB:CC:DD:EE:FF");
        assert_eq!(e.chan, 6);
        assert_eq!(e.freq, 2437);
        assert_eq!(e.rate, 130);
        assert_eq!(e.signal, 70);
        assert_eq!(e.security, "WPA2");
        assert!(parse_scan_entry("garbage").is_err());
    }

    #[test]
    fn details_parse_like_python_nmcli() {
        let text = "GENERAL.DEVICE:                         wlan0\nGENERAL.CONNECTION:                     --\nIP4.ADDRESS[1]:                         192.168.1.42/24\n\nGENERAL.DEVICE:                         lo\nGENERAL.HWADDR:                         \"\"\n";
        let all = parse_details_all(text);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].get("GENERAL.DEVICE"), Some("wlan0"));
        assert_eq!(all[0].get("GENERAL.CONNECTION"), None);
        assert_eq!(all[0].get("IP4.ADDRESS[1]"), Some("192.168.1.42/24"));
        assert_eq!(all[1].0[1], ("GENERAL.HWADDR".to_string(), None));
        assert_eq!(parse_details(text).get("GENERAL.DEVICE"), Some("lo"));
    }

    #[test]
    fn exit_codes_raise_the_package_exceptions() {
        assert_eq!(exception_for(4, "").message, "Connection activation failed");
        assert_eq!(
            exception_for(4, "").kind,
            "ConnectionActivateFailedException"
        );
        assert_eq!(
            exception_for(1, "Error: boom").message,
            "Unknown or unspecified error [code:1, detail:Error: boom]"
        );
        assert_eq!(
            exception_for(1, "Error: Scanning not allowed").kind,
            "ScanningNotAllowedException"
        );
    }
}
