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
        // Deliberate fix: python-nmcli parses the padded table `nmcli
        // connection` prints and strips each name, so a profile named
        // " Home" or "Home " was listed as "Home", and `connection up` or
        // `delete` of that name exited 10 (NotExistException). Terse output
        // keeps every character of the name.
        let out = self.nmcli(&["-t", "-f", "NAME,UUID,TYPE,DEVICE", "connection", "show"])?;
        out.split('\n')
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

/// The values of a terse (`-t`) row: nmcli separates them with `:` and puts
/// a `\` before each `:` or `\` inside a value.
fn terse_values(row: &str) -> Vec<String> {
    let mut values = vec![String::new()];
    let mut chars = row.chars();
    while let Some(c) = chars.next() {
        match c {
            ':' => values.push(String::new()),
            '\\' => {
                let escaped = chars.next().unwrap_or('\\');
                values.last_mut().expect("never empty").push(escaped);
            }
            c => values.last_mut().expect("never empty").push(c),
        }
    }
    values
}

/// A row of `nmcli -t -f NAME,UUID,TYPE,DEVICE connection show`, as
/// `Connection.parse` reads the table row: a Wi-Fi profile's type is the
/// alias the table prints (`wifi`, not `802-11-wireless`) and no device is
/// `--`.
pub fn parse_connection(text: &str) -> PyResult<Connection> {
    let [name, uuid, conn_type, device]: [String; 4] = terse_values(text)
        .try_into()
        .map_err(|_| parse_failed(text))?;
    Ok(Connection {
        name,
        uuid,
        conn_type: if conn_type == "802-11-wireless" {
            "wifi".to_string()
        } else {
            conn_type
        },
        device: if device.is_empty() {
            "--".to_string()
        } else {
            device
        },
    })
}

fn detail_value(value: &str) -> Option<String> {
    (value != "--" && value != "\"\"").then(|| value.to_string())
}

/// The column nmcli pads a multiline row's `KEY:` to (`ML_VALUE_INDENT`).
const VALUE_COLUMN: usize = 40;

/// The key and value of a `KEY:   value` row of `nmcli device show`.
fn detail_row(row: &str) -> Option<(&str, &str)> {
    let key = DETAIL_ROW.captures(row)?.get(1)?.as_str();
    // Deliberate fix: python-nmcli's `:\s*` also eats the value's own leading
    // spaces, so the device of a profile named " Home" reported "Home" and
    // the joined network never matched. Only nmcli's padding is dropped.
    let rest = &row[key.len() + 1..];
    let padding = VALUE_COLUMN.saturating_sub(key.len() + 1);
    let skip = rest
        .bytes()
        .take(padding)
        .take_while(|b| *b == b' ')
        .count();
    Some((key, &rest[skip..]))
}

/// `DeviceControl.show`'s parsing.
pub fn parse_details(text: &str) -> DeviceDetails {
    let mut details = DeviceDetails::default();
    for row in text.split('\n') {
        if let Some((key, value)) = detail_row(row) {
            details.insert(key, detail_value(value));
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
        let Some((key, value)) = detail_row(row) else {
            continue;
        };
        if key == "GENERAL.DEVICE" {
            all.push(DeviceDetails::default());
            current = Some(all.len() - 1);
        }
        let target = match current {
            Some(i) => &mut all[i],
            // Rows before the first device go to a dict nobody keeps.
            None => &mut orphan,
        };
        target.insert(key, detail_value(value));
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

    /// Saved profiles keep their exact names, so Known Wi-Fis can connect to
    /// and forget a network named " Home", "Cafe " or `a:b\c`; trimmed, the
    /// name matched no profile and nmcli exited 10. The rows are nmcli
    /// 1.42's terse output (`src/nmcli/utils.c` escapes `:` and `\` with
    /// `\`; `connections.c` prints the setting name, not the `wifi` alias,
    /// outside pretty mode, and nothing for no device).
    #[test]
    fn connection_rows_keep_the_exact_profile_names() {
        let rows = [
            " Home:0f6b8c1e-1111-2222-3333-444455556666:802-11-wireless:wlan0",
            "Cafe :1a2b3c4d-1111-2222-3333-444455556666:802-11-wireless:",
            "a\\:b\\\\c:5e6f7a8b-1111-2222-3333-444455556666:802-11-wireless:",
            "lo:9c0d1e2f-1111-2222-3333-444455556666:loopback:lo",
        ];
        let stdout = rows.join("\n") + "\n";
        let runner = crate::py::fake::RecordingRunner::new(move |_, _| {
            Ok(crate::py::Completed {
                stdout: stdout.clone(),
                ..Default::default()
            })
        });

        let connections = NmcliCli::with_runner(runner.clone())
            .connections()
            .expect("listed");

        assert_eq!(
            runner.calls(),
            vec![(
                "nmcli -t -f NAME,UUID,TYPE,DEVICE connection show".to_string(),
                false
            )]
        );
        let fields: Vec<(&str, &str, &str, &str)> = connections
            .iter()
            .map(|c| (&*c.name, &*c.uuid, &*c.conn_type, &*c.device))
            .collect();
        assert_eq!(
            fields,
            vec![
                (
                    " Home",
                    "0f6b8c1e-1111-2222-3333-444455556666",
                    "wifi",
                    "wlan0"
                ),
                (
                    "Cafe ",
                    "1a2b3c4d-1111-2222-3333-444455556666",
                    "wifi",
                    "--"
                ),
                (
                    "a:b\\c",
                    "5e6f7a8b-1111-2222-3333-444455556666",
                    "wifi",
                    "--"
                ),
                (
                    "lo",
                    "9c0d1e2f-1111-2222-3333-444455556666",
                    "loopback",
                    "lo"
                ),
            ]
        );
        assert_eq!(
            parse_connection("a:b:uuid:802-11-wireless:").unwrap_err(),
            parse_failed("a:b:uuid:802-11-wireless:")
        );
    }

    /// The connection a device reports keeps its own leading and trailing
    /// spaces, so after joining " Home " the backend sees " Home " connected
    /// instead of reporting the join preempted. nmcli 1.42 pads a multiline
    /// `KEY:` to column 40 (`ML_VALUE_INDENT`, `src/nmcli/utils.c`); the
    /// rest of the row is the value.
    #[test]
    fn device_details_keep_the_spaces_of_a_connection_name() {
        let row = |key: &str, value: &str| format!("{:<40}{value}", format!("{key}:"));
        let text = [
            row("GENERAL.DEVICE", "wlan0"),
            row("GENERAL.CONNECTION", " Home "),
            row("IP4.ADDRESS[1]", "192.168.1.42/24"),
        ]
        .join("\n");

        assert_eq!(
            parse_details(&text).get("GENERAL.CONNECTION"),
            Some(" Home ")
        );
        assert_eq!(
            parse_details_all(&text)[0].get("GENERAL.CONNECTION"),
            Some(" Home ")
        );
        assert_eq!(
            parse_details(&text).get("IP4.ADDRESS[1]"),
            Some("192.168.1.42/24")
        );
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
