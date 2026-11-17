//! Improv Wi-Fi over BLE, as the `pyImprov` package (0.2.0) implements it:
//! the service and characteristic UUIDs, the RPC framing (command, length,
//! payload, 8-bit checksum), the states and errors, and the response
//! packets, split to fit `max_response_bytes`.
//!
//! The package's quirks stay: `IDENTIFY` and `GET_CURRENT_STATE` share the
//! value 0x02, so 0x02 is always `IDENTIFY`; and a response packet is only
//! dropped, never truncated, when it would be too long.

use crate::py::PyError;

pub const SERVICE_UUID: &str = "00467768-6228-2272-4663-277478268000";
pub const STATUS_UUID: &str = "00467768-6228-2272-4663-277478268001";
pub const ERROR_UUID: &str = "00467768-6228-2272-4663-277478268002";
pub const RPC_COMMAND_UUID: &str = "00467768-6228-2272-4663-277478268003";
pub const RPC_RESULT_UUID: &str = "00467768-6228-2272-4663-277478268004";
pub const CAPABILITIES_UUID: &str = "00467768-6228-2272-4663-277478268005";

/// `ImprovUUID(uuid).name`.
pub fn uuid_name(uuid: &str) -> Option<&'static str> {
    Some(match uuid {
        SERVICE_UUID => "SERVICE_UUID",
        STATUS_UUID => "STATUS_UUID",
        ERROR_UUID => "ERROR_UUID",
        RPC_COMMAND_UUID => "RPC_COMMAND_UUID",
        RPC_RESULT_UUID => "RPC_RESULT_UUID",
        CAPABILITIES_UUID => "CAPABILITIES_UUID",
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[allow(
    dead_code,
    reason = "STOPPED is a protocol value the device never reports"
)]
pub enum State {
    Stopped = 0x00,
    AwaitingAuthorization = 0x01,
    Authorized = 0x02,
    Provisioning = 0x03,
    Provisioned = 0x04,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImprovError {
    None = 0x00,
    InvalidRpc = 0x01,
    UnknownRpc = 0x02,
    UnableToConnect = 0x03,
    NotAuthorized = 0x04,
    Unknown = 0xFF,
}

impl ImprovError {
    fn py_name(self) -> &'static str {
        match self {
            ImprovError::None => "ImprovError.NONE",
            ImprovError::InvalidRpc => "ImprovError.INVALID_RPC",
            ImprovError::UnknownRpc => "ImprovError.UNKNOWN_RPC",
            ImprovError::UnableToConnect => "ImprovError.UNABLE_TO_CONNECT",
            ImprovError::NotAuthorized => "ImprovError.NOT_AUTHORIZED",
            ImprovError::Unknown => "ImprovError.UNKNOWN",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Unknown = 0x00,
    WifiSettings = 0x01,
    /// Also `GET_CURRENT_STATE`, which Python's enum aliases to it.
    Identify = 0x02,
    GetDeviceInfo = 0x03,
    GetWifiNetworks = 0x04,
    BadChecksum = 0xFF,
}

impl Command {
    fn from_byte(byte: u8) -> Option<Command> {
        Some(match byte {
            0x00 => Command::Unknown,
            0x01 => Command::WifiSettings,
            0x02 => Command::Identify,
            0x03 => Command::GetDeviceInfo,
            0x04 => Command::GetWifiNetworks,
            0xFF => Command::BadChecksum,
            _ => return None,
        })
    }

    fn py_name(self) -> &'static str {
        match self {
            Command::Unknown => "ImprovCommand.UNKNOWN",
            Command::WifiSettings => "ImprovCommand.WIFI_SETTINGS",
            Command::Identify => "ImprovCommand.IDENTIFY",
            Command::GetDeviceInfo => "ImprovCommand.GET_DEVICE_INFO",
            Command::GetWifiNetworks => "ImprovCommand.GET_WIFI_NETWORKS",
            Command::BadChecksum => "ImprovCommand.BAD_CHECKSUM",
        }
    }
}

/// A characteristic value: one buffer, or the packets of an RPC response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Payload {
    Bytes(Vec<u8>),
    Packets(Vec<Vec<u8>>),
}

impl Payload {
    /// What `ble_gatt` hands to BlueZ for a read: the first packet of a list.
    pub fn first(&self) -> Result<Vec<u8>, PyError> {
        match self {
            Payload::Bytes(bytes) => Ok(bytes.clone()),
            Payload::Packets(packets) => packets
                .first()
                .cloned()
                .ok_or_else(|| PyError::new("IndexError", "list index out of range")),
        }
    }
}

/// `parse_improv_data`'s result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub command: Command,
    pub credentials: Option<(Vec<u8>, Vec<u8>)>,
}

impl Parsed {
    fn only(command: Command) -> Parsed {
        Parsed {
            command,
            credentials: None,
        }
    }
}

/// The callbacks `ImprovProtocol` was built with.
pub trait Callbacks {
    /// `wifi_connect_callback(ssid, password)`: the device's URLs, `None` on
    /// failure.
    fn wifi_connect(&mut self, ssid: &[u8], password: &[u8]) -> Option<Vec<String>>;
    /// `wifi_networks_callback()`.
    fn wifi_networks(&mut self) -> Option<Vec<String>>;
}

pub struct ImprovProtocol {
    pub state: State,
    pub last_error: ImprovError,
    pub rpc_response: Payload,
    max_response_bytes: usize,
}

/// `calculateChecksum`.
pub fn checksum(data: &[u8]) -> u8 {
    data.iter().fold(0u32, |acc, b| acc + u32::from(*b)) as u8
}

impl ImprovProtocol {
    /// `ImprovProtocol(requires_authorization=..., identify_callback=None,
    /// device_info_callback=None, max_response_bytes=...)`.
    pub fn new(requires_authorization: bool, max_response_bytes: usize) -> ImprovProtocol {
        ImprovProtocol {
            state: if requires_authorization {
                State::AwaitingAuthorization
            } else {
                State::Authorized
            },
            last_error: ImprovError::None,
            rpc_response: Payload::Bytes(Vec::new()),
            max_response_bytes,
        }
    }

    /// `parse_improv_data(data)`.
    pub fn parse(&self, data: &[u8]) -> Result<Parsed, PyError> {
        let first = *data
            .first()
            .ok_or_else(|| PyError::new("IndexError", "bytearray index out of range"))?;
        let Some(command) = Command::from_byte(first) else {
            return Ok(Parsed::only(Command::Unknown));
        };
        tracing::info!("Command recieved: {}", command.py_name());
        if data.len() == 1 {
            if command == Command::WifiSettings {
                tracing::warn!("WIFI settings command without payload");
                return Ok(Parsed::only(Command::Unknown));
            }
            return Ok(Parsed::only(command));
        }
        let length = i64::from(data[1]);
        let expected = data.len() as i64 - 3;
        if length != expected {
            tracing::warn!("length mismatch: {length}  != {expected}");
            return Ok(Parsed::only(Command::Unknown));
        }
        let received = data[data.len() - 1];
        let calculated = checksum(&data[..data.len() - 1]);
        if calculated != received {
            tracing::warn!("Checksums are {:#x} and {:#x}", received, calculated);
            return Ok(Parsed::only(Command::BadChecksum));
        }
        if command == Command::WifiSettings {
            let ssid_end = 3 + usize::from(data[2]);
            // At least one byte must remain for the password length.
            if ssid_end >= data.len() - 1 {
                return Ok(Parsed::only(command));
            }
            let ssid = data[3..ssid_end].to_vec();
            let password_length = usize::from(data[ssid_end]);
            let password_start = ssid_end + 1;
            if password_start + password_length >= data.len() {
                return Ok(Parsed::only(command));
            }
            let password = data[password_start..password_start + password_length].to_vec();
            return Ok(Parsed {
                command,
                credentials: Some((ssid, password)),
            });
        }
        Ok(Parsed::only(command))
    }

    /// `build_rpc_response(command, data)`.
    pub fn build_rpc_response(
        &self,
        command: Command,
        data: &[Vec<u8>],
    ) -> Result<Vec<Vec<u8>>, PyError> {
        fn finish(response: &mut Vec<u8>) -> Result<(), PyError> {
            let length = u8::try_from(response.len() - 2)
                .map_err(|_| PyError::value_error("byte must be in range(0, 256)"))?;
            response[1] = length;
            let sum = checksum(response);
            response.push(sum);
            Ok(())
        }
        let mut responses = Vec::new();
        let mut current = vec![command as u8, 0];
        for component in data {
            if current.len() - 2 + 1 + component.len() > self.max_response_bytes {
                finish(&mut current)?;
                if current.len() <= self.max_response_bytes {
                    responses.push(current);
                }
                current = vec![command as u8, 0];
            }
            let length = u8::try_from(component.len())
                .map_err(|_| PyError::new("OverflowError", "int too big to convert"))?;
            current.push(length);
            current.extend_from_slice(component);
        }
        finish(&mut current)?;
        responses.push(current);
        Ok(responses)
    }

    /// `handle_read(uuid)`.
    pub fn handle_read(&self, uuid: &str) -> Payload {
        match uuid {
            STATUS_UUID => Payload::Bytes(vec![self.state as u8]),
            // With or without an identify callback the package says 0x01.
            CAPABILITIES_UUID => Payload::Bytes(vec![0x01]),
            ERROR_UUID => Payload::Bytes(vec![self.last_error as u8]),
            RPC_RESULT_UUID => self.rpc_response.clone(),
            _ => Payload::Bytes(Vec::new()),
        }
    }

    /// `handle_write(uuid, data)`: the characteristic to notify and its
    /// value, `None` for a characteristic that is not the RPC command.
    pub fn handle_write(
        &mut self,
        uuid: &str,
        data: &[u8],
        callbacks: &mut dyn Callbacks,
    ) -> Result<Option<(&'static str, Payload)>, PyError> {
        if uuid != RPC_COMMAND_UUID {
            return Ok(None);
        }
        self.last_error = ImprovError::None;
        let parsed = self.parse(data)?;
        match parsed.command {
            Command::BadChecksum => self.last_error = ImprovError::InvalidRpc,
            Command::WifiSettings => {
                if self.state < State::Authorized {
                    self.last_error = ImprovError::NotAuthorized;
                } else if let Some((ssid, password)) = &parsed.credentials {
                    self.state = State::Provisioning;
                    match callbacks.wifi_connect(ssid, password) {
                        Some(urls) => {
                            self.state = State::Provisioned;
                            let data: Vec<Vec<u8>> =
                                urls.into_iter().map(String::into_bytes).collect();
                            self.rpc_response = Payload::Packets(
                                self.build_rpc_response(Command::WifiSettings, &data)?,
                            );
                        }
                        None => {
                            self.state = State::Authorized;
                            self.last_error = ImprovError::UnableToConnect;
                        }
                    }
                } else {
                    self.last_error = ImprovError::InvalidRpc;
                }
            }
            // No identify callback.
            Command::Identify => self.last_error = ImprovError::InvalidRpc,
            Command::GetDeviceInfo => {
                tracing::warn!("Client requested GET_DEVICE_INFO but it is not implemented");
                self.last_error = ImprovError::UnknownRpc;
            }
            Command::GetWifiNetworks => match callbacks.wifi_networks() {
                Some(networks) => {
                    let data: Vec<Vec<u8>> = networks.into_iter().map(String::into_bytes).collect();
                    self.rpc_response =
                        Payload::Packets(self.build_rpc_response(Command::GetWifiNetworks, &data)?);
                }
                None => self.last_error = ImprovError::Unknown,
            },
            Command::Unknown => self.last_error = ImprovError::UnknownRpc,
        }
        if self.last_error != ImprovError::None {
            tracing::warn!(
                "An error occured during execution: {}",
                self.last_error.py_name()
            );
            return Ok(Some((
                ERROR_UUID,
                Payload::Bytes(vec![self.last_error as u8]),
            )));
        }
        tracing::debug!("RPC response: {:?}", self.rpc_response);
        Ok(Some((RPC_RESULT_UUID, self.rpc_response.clone())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        urls: Option<Vec<String>>,
        networks: Option<Vec<String>>,
        joined: Vec<(Vec<u8>, Vec<u8>)>,
    }

    impl Callbacks for Fake {
        fn wifi_connect(&mut self, ssid: &[u8], password: &[u8]) -> Option<Vec<String>> {
            self.joined.push((ssid.to_vec(), password.to_vec()));
            self.urls.clone()
        }
        fn wifi_networks(&mut self) -> Option<Vec<String>> {
            self.networks.clone()
        }
    }

    fn fake() -> Fake {
        Fake {
            urls: Some(vec!["http://192.168.1.42:8080".into()]),
            networks: Some(vec!["HomeNet".into(), "Cafe".into()]),
            joined: Vec::new(),
        }
    }

    fn frame(command: u8, payload: &[u8]) -> Vec<u8> {
        let mut data = vec![command, payload.len() as u8];
        data.extend_from_slice(payload);
        data.push(checksum(&data));
        data
    }

    fn wifi_settings(ssid: &[u8], password: &[u8]) -> Vec<u8> {
        let mut payload = vec![ssid.len() as u8];
        payload.extend_from_slice(ssid);
        payload.push(password.len() as u8);
        payload.extend_from_slice(password);
        frame(0x01, &payload)
    }

    #[test]
    fn wifi_settings_provision_and_answer_with_the_urls() {
        let mut improv = ImprovProtocol::new(false, 250);
        let mut callbacks = fake();
        let (uuid, value) = improv
            .handle_write(
                RPC_COMMAND_UUID,
                &wifi_settings(b"HomeNet", b"hunter22"),
                &mut callbacks,
            )
            .expect("no exception")
            .expect("answer");
        assert_eq!(uuid, RPC_RESULT_UUID);
        assert_eq!(
            callbacks.joined,
            vec![(b"HomeNet".to_vec(), b"hunter22".to_vec())]
        );
        assert_eq!(improv.state, State::Provisioned);
        let url = b"http://192.168.1.42:8080";
        let mut expected = vec![0x01, (url.len() + 1) as u8, url.len() as u8];
        expected.extend_from_slice(url);
        expected.push(checksum(&expected));
        assert_eq!(value, Payload::Packets(vec![expected]));
    }

    #[test]
    fn failed_join_reports_unable_to_connect() {
        let mut improv = ImprovProtocol::new(false, 250);
        let mut callbacks = fake();
        callbacks.urls = None;
        let answer = improv
            .handle_write(RPC_COMMAND_UUID, &wifi_settings(b"X", b"y"), &mut callbacks)
            .expect("no exception");
        assert_eq!(answer, Some((ERROR_UUID, Payload::Bytes(vec![0x03]))));
        assert_eq!(improv.state, State::Authorized);
    }

    #[test]
    fn framing_errors() {
        let mut improv = ImprovProtocol::new(false, 250);
        let mut callbacks = fake();
        let mut bad = wifi_settings(b"X", b"y");
        let last = bad.len() - 1;
        bad[last] ^= 0xFF;
        assert_eq!(
            improv
                .handle_write(RPC_COMMAND_UUID, &bad, &mut callbacks)
                .expect("ok"),
            Some((ERROR_UUID, Payload::Bytes(vec![0x01])))
        );
        // Length mismatch parses as UNKNOWN.
        assert_eq!(
            improv
                .handle_write(RPC_COMMAND_UUID, &[0x01, 9, 0], &mut callbacks)
                .expect("ok"),
            Some((ERROR_UUID, Payload::Bytes(vec![0x02])))
        );
        // 0x02 is IDENTIFY, which has no callback.
        assert_eq!(
            improv
                .handle_write(RPC_COMMAND_UUID, &[0x02], &mut callbacks)
                .expect("ok"),
            Some((ERROR_UUID, Payload::Bytes(vec![0x01])))
        );
        assert_eq!(
            improv
                .handle_write(RPC_COMMAND_UUID, &[0x03], &mut callbacks)
                .expect("ok"),
            Some((ERROR_UUID, Payload::Bytes(vec![0x02])))
        );
        assert!(
            improv
                .handle_write(RPC_COMMAND_UUID, &[], &mut callbacks)
                .is_err()
        );
        assert_eq!(
            improv
                .handle_write(STATUS_UUID, &[0x01], &mut callbacks)
                .expect("ok"),
            None
        );
        assert_eq!(
            improv.parse(&[0x01]).expect("ok"),
            Parsed::only(Command::Unknown)
        );
    }

    #[test]
    fn network_list_is_split_into_packets() {
        let mut improv = ImprovProtocol::new(false, 10);
        let mut callbacks = fake();
        callbacks.networks = Some(vec!["AA".into(), "BB".into(), "CCCC".into()]);
        let (_, value) = improv
            .handle_write(RPC_COMMAND_UUID, &[0x04], &mut callbacks)
            .expect("ok")
            .expect("answer");
        let Payload::Packets(packets) = value else {
            panic!("packets");
        };
        assert_eq!(packets.len(), 2);
        assert_eq!(&packets[0][..8], &[0x04, 6, 2, b'A', b'A', 2, b'B', b'B']);
        assert_eq!(&packets[1][..7], &[0x04, 5, 4, b'C', b'C', b'C', b'C']);
        assert_eq!(
            improv.handle_read(RPC_RESULT_UUID),
            Payload::Packets(packets)
        );

        // pyImprov drops a full packet instead of sending one that is too long.
        callbacks.networks = Some(vec!["AAAA".into(), "BBBB".into(), "C".into()]);
        let (_, value) = improv
            .handle_write(RPC_COMMAND_UUID, &[0x04], &mut callbacks)
            .expect("ok")
            .expect("answer");
        assert_eq!(
            value,
            Payload::Packets(vec![vec![0x04, 2, 1, b'C', 0x04 + 2 + 1 + b'C']])
        );
    }

    #[test]
    fn reads() {
        let improv = ImprovProtocol::new(false, 250);
        assert_eq!(improv.handle_read(STATUS_UUID), Payload::Bytes(vec![0x02]));
        assert_eq!(
            improv.handle_read(CAPABILITIES_UUID),
            Payload::Bytes(vec![0x01])
        );
        assert_eq!(improv.handle_read(ERROR_UUID), Payload::Bytes(vec![0x00]));
        assert_eq!(improv.handle_read(RPC_RESULT_UUID), Payload::Bytes(vec![]));
        assert_eq!(
            ImprovProtocol::new(true, 250).state,
            State::AwaitingAuthorization
        );
    }
}
