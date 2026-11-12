//! Improv Wi-Fi over BLE, as the `pyImprov` package (0.2.0) implements it:
//! the service and characteristic UUIDs, the RPC framing (command, length,
//! payload, 8-bit checksum), the states and errors, and the response
//! packets, split to fit `max_response_bytes`.
//!
//! The package's quirks stay: `IDENTIFY` and `GET_CURRENT_STATE` share the
//! value 0x02, so 0x02 is always `IDENTIFY`; and a response packet is only
//! dropped, never truncated, when it would be too long.
//!
//! Unlike the Python:
//! - `IDENTIFY` runs [`Callbacks::identify`] and answers with an empty RPC
//!   result (the Python had no identify callback: `INVALID_RPC`).
//! - `WIFI_SETTINGS` joins nothing until someone answers on the machine:
//!   the state drops to `AUTHORIZATION_REQUIRED` while the Dial asks, then
//!   [`ImprovProtocol::answered`] and [`ImprovProtocol::joined`] carry the
//!   request through. The idle state stays `AUTHORIZED`, as app versions
//!   that only send credentials once authorized expect.
//! - The `WIFI_SETTINGS` result fits one notification at the MTU the app
//!   asks for ([`MAX_RESULT_PACKET_BYTES`]): the first URL, then a pairing
//!   URL (`<first URL>/#pair=<code>`) for the phone that set the machine
//!   up, then as many of the other URLs as fit. Old apps only read the
//!   first entry of the first notification. The copy a later read returns
//!   holds the URLs only, so the code is never read back. BlueZ sends the
//!   notification to every connected central that subscribed, so the
//!   server first disconnects every other connected BLE central (again if
//!   one comes back) and passes a pairing URL only once BlueZ lists the
//!   writer alone; once BlueZ took the notification it lists them again
//!   and revokes the code if anyone else is connected. Left open: a
//!   central that gets the notification and is gone again before that
//!   second listing, or that exchanges the code first; and, where BlueZ
//!   does not tell LE from Classic links, a Classic one (audio) counts as
//!   a central.

use crate::py::PyError;

pub const SERVICE_UUID: &str = "00467768-6228-2272-4663-277478268000";
pub const STATUS_UUID: &str = "00467768-6228-2272-4663-277478268001";
pub const ERROR_UUID: &str = "00467768-6228-2272-4663-277478268002";
pub const RPC_COMMAND_UUID: &str = "00467768-6228-2272-4663-277478268003";
pub const RPC_RESULT_UUID: &str = "00467768-6228-2272-4663-277478268004";
pub const CAPABILITIES_UUID: &str = "00467768-6228-2272-4663-277478268005";

/// The longest RPC result packet one notification carries: the app asks
/// for an MTU of 185, and ATT takes 3 bytes of it.
pub const MAX_RESULT_PACKET_BYTES: usize = 182;

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
    /// Ask on the machine whether to join this network. The answer comes
    /// back later, through [`ImprovProtocol::answered`].
    fn request_authorization(&mut self, ssid: &[u8], password: &[u8]);
    /// `wifi_networks_callback()`.
    fn wifi_networks(&mut self) -> Option<Vec<String>>;
    /// `identify_callback()`: make the device show itself. The command
    /// carries no payload, so who asked comes from the BLE link, not here.
    fn identify(&mut self);
}

pub struct ImprovProtocol {
    pub state: State,
    pub last_error: ImprovError,
    pub rpc_response: Payload,
    max_response_bytes: usize,
}

/// The first entry, then the following ones while the RPC result packet
/// (command, length, a length byte per entry, checksum) stays within
/// [`MAX_RESULT_PACKET_BYTES`]: the app reads the first notification only.
fn fit_one_packet(entries: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
    let mut kept = Vec::new();
    let mut size = 3;
    for entry in entries {
        let grown = size + 1 + entry.len();
        if !kept.is_empty() && grown > MAX_RESULT_PACKET_BYTES {
            break;
        }
        size = grown;
        kept.push(entry);
    }
    kept
}

/// `calculateChecksum`.
pub fn checksum(data: &[u8]) -> u8 {
    data.iter().fold(0u32, |acc, b| acc + u32::from(*b)) as u8
}

impl ImprovProtocol {
    /// `ImprovProtocol(requires_authorization=..., identify_callback=...,
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

    fn status(&self) -> Payload {
        Payload::Bytes(vec![self.state as u8])
    }

    /// Whether a `WIFI_SETTINGS` is waiting for its answer or joining.
    pub fn request_in_flight(&self) -> bool {
        matches!(
            self.state,
            State::AwaitingAuthorization | State::Provisioning
        )
    }

    /// The machine's answer to the pending `WIFI_SETTINGS`, as the
    /// characteristics to notify in order. Approved: `AUTHORIZED`, then
    /// `PROVISIONING` while the machine joins ([`ImprovProtocol::joined`]
    /// follows). Rejected or unanswered: `NOT_AUTHORIZED`, and `AUTHORIZED`
    /// again, ready for the next request.
    pub fn answered(&mut self, approved: bool) -> Vec<(&'static str, Payload)> {
        self.state = State::Authorized;
        if !approved {
            self.last_error = ImprovError::NotAuthorized;
            return vec![
                (ERROR_UUID, Payload::Bytes(vec![self.last_error as u8])),
                (STATUS_UUID, self.status()),
            ];
        }
        let authorized = (STATUS_UUID, self.status());
        self.state = State::Provisioning;
        vec![authorized, (STATUS_UUID, self.status())]
    }

    /// The approved join's outcome (the device's URLs, `None` when it
    /// failed), as the characteristics to notify in order. `pair_url` goes
    /// second in the notified result only.
    pub fn joined(
        &mut self,
        urls: Option<Vec<String>>,
        pair_url: Option<String>,
    ) -> Result<Vec<(&'static str, Payload)>, PyError> {
        let Some(urls) = urls else {
            self.state = State::Authorized;
            self.last_error = ImprovError::UnableToConnect;
            return Ok(vec![
                (ERROR_UUID, Payload::Bytes(vec![self.last_error as u8])),
                (STATUS_UUID, self.status()),
            ]);
        };
        let urls: Vec<Vec<u8>> = urls.into_iter().map(String::into_bytes).collect();
        let mut notified = urls.clone();
        if let Some(pair_url) = pair_url
            && !notified.is_empty()
        {
            notified.insert(1, pair_url.into_bytes());
        }
        let stored = self.build_rpc_response(Command::WifiSettings, &fit_one_packet(urls))?;
        let notified = self.build_rpc_response(Command::WifiSettings, &fit_one_packet(notified))?;
        self.rpc_response = Payload::Packets(stored);
        self.state = State::Provisioned;
        Ok(vec![
            (STATUS_UUID, self.status()),
            (RPC_RESULT_UUID, Payload::Packets(notified)),
        ])
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
                // Not while another request waits for its answer or joins.
                if !matches!(self.state, State::Authorized | State::Provisioned) {
                    self.last_error = ImprovError::NotAuthorized;
                } else if let Some((ssid, password)) = &parsed.credentials {
                    self.state = State::AwaitingAuthorization;
                    callbacks.request_authorization(ssid, password);
                    return Ok(Some((STATUS_UUID, self.status())));
                } else {
                    self.last_error = ImprovError::InvalidRpc;
                }
            }
            Command::Identify => {
                callbacks.identify();
                // An empty result, so the client sees the command was handled
                // instead of the previous command's result again.
                self.rpc_response =
                    Payload::Packets(self.build_rpc_response(Command::Identify, &[])?);
            }
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
        networks: Option<Vec<String>>,
        requested: Vec<(Vec<u8>, Vec<u8>)>,
        identified: usize,
    }

    impl Callbacks for Fake {
        fn request_authorization(&mut self, ssid: &[u8], password: &[u8]) {
            self.requested.push((ssid.to_vec(), password.to_vec()));
        }
        fn wifi_networks(&mut self) -> Option<Vec<String>> {
            self.networks.clone()
        }
        fn identify(&mut self) {
            self.identified += 1;
        }
    }

    fn fake() -> Fake {
        Fake {
            networks: Some(vec!["HomeNet".into(), "Cafe".into()]),
            requested: Vec::new(),
            identified: 0,
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

    /// WIFI_SETTINGS joins nothing until the machine answers: the state
    /// drops to AUTHORIZATION_REQUIRED (the Improv BLE spec's 0x01) and the
    /// credentials go to the callback that asks on the Dial.
    #[test]
    fn wifi_settings_wait_for_the_answer() {
        let mut improv = ImprovProtocol::new(false, 250);
        let mut callbacks = fake();
        let answer = improv
            .handle_write(
                RPC_COMMAND_UUID,
                &wifi_settings(b"HomeNet", b"hunter22"),
                &mut callbacks,
            )
            .expect("no exception");
        assert_eq!(answer, Some((STATUS_UUID, Payload::Bytes(vec![0x01]))));
        assert_eq!(
            callbacks.requested,
            vec![(b"HomeNet".to_vec(), b"hunter22".to_vec())]
        );
        assert!(improv.request_in_flight());
        // A second request while the first waits is refused, not queued.
        let answer = improv
            .handle_write(RPC_COMMAND_UUID, &wifi_settings(b"X", b"y"), &mut callbacks)
            .expect("no exception");
        assert_eq!(answer, Some((ERROR_UUID, Payload::Bytes(vec![0x04]))));
        assert_eq!(callbacks.requested.len(), 1);
    }

    /// Approved and joined: AUTHORIZED, PROVISIONING, PROVISIONED, then the
    /// URL packets, which the app waits for.
    #[test]
    fn approved_request_provisions_and_answers_with_the_urls() {
        let mut improv = ImprovProtocol::new(false, 250);
        improv
            .handle_write(
                RPC_COMMAND_UUID,
                &wifi_settings(b"HomeNet", b"hunter22"),
                &mut fake(),
            )
            .expect("no exception");
        assert_eq!(
            improv.answered(true),
            vec![
                (STATUS_UUID, Payload::Bytes(vec![0x02])),
                (STATUS_UUID, Payload::Bytes(vec![0x03])),
            ]
        );
        let url = b"http://192.168.1.42:8080";
        let mut expected = vec![0x01, (url.len() + 1) as u8, url.len() as u8];
        expected.extend_from_slice(url);
        expected.push(checksum(&expected));
        assert_eq!(
            improv
                .joined(Some(vec!["http://192.168.1.42:8080".into()]), None)
                .expect("built"),
            vec![
                (STATUS_UUID, Payload::Bytes(vec![0x04])),
                (RPC_RESULT_UUID, Payload::Packets(vec![expected])),
            ]
        );
        assert_eq!(improv.state, State::Provisioned);
        assert!(!improv.request_in_flight());
    }

    /// Rejected (or unanswered): NOT_AUTHORIZED, then AUTHORIZED again so the
    /// app can send the next request; a failed join: UNABLE_TO_CONNECT.
    #[test]
    fn rejected_request_and_failed_join_report_the_error() {
        let mut improv = ImprovProtocol::new(false, 250);
        improv
            .handle_write(RPC_COMMAND_UUID, &wifi_settings(b"X", b"y"), &mut fake())
            .expect("no exception");
        assert_eq!(
            improv.answered(false),
            vec![
                (ERROR_UUID, Payload::Bytes(vec![0x04])),
                (STATUS_UUID, Payload::Bytes(vec![0x02])),
            ]
        );
        assert_eq!(improv.state, State::Authorized);

        improv
            .handle_write(RPC_COMMAND_UUID, &wifi_settings(b"X", b"y"), &mut fake())
            .expect("no exception");
        improv.answered(true);
        assert_eq!(
            improv.joined(None, None).expect("built"),
            vec![
                (ERROR_UUID, Payload::Bytes(vec![0x03])),
                (STATUS_UUID, Payload::Bytes(vec![0x02])),
            ]
        );
        assert_eq!(improv.state, State::Authorized);
    }

    /// The entries of one RPC result packet, after checking its framing.
    fn entries(packet: &[u8]) -> Vec<String> {
        assert_eq!(usize::from(packet[1]), packet.len() - 3, "length byte");
        assert_eq!(
            checksum(&packet[..packet.len() - 1]),
            packet[packet.len() - 1]
        );
        let mut entries = Vec::new();
        let mut at = 2;
        while at < packet.len() - 1 {
            let length = usize::from(packet[at]);
            entries
                .push(String::from_utf8(packet[at + 1..at + 1 + length].to_vec()).expect("utf8"));
            at += 1 + length;
        }
        entries
    }

    fn single_packet(payload: &Payload) -> Vec<u8> {
        let Payload::Packets(packets) = payload else {
            panic!("packets");
        };
        assert_eq!(packets.len(), 1, "one packet");
        packets[0].clone()
    }

    /// The approved join's result fits one notification at the app's MTU
    /// (185 - 3 = MAX_RESULT_PACKET_BYTES): the base URL first, as old apps
    /// read it, the pair URL second, then the other URLs while they fit;
    /// the copy a later read returns has no pair URL, so no later read
    /// gets the code (device authorization contract, section 8). Who is
    /// notified is the server's call (`ble::tests`).
    #[test]
    fn joined_result_carries_the_pair_url_in_one_packet() {
        let mut improv = ImprovProtocol::new(false, 250);
        improv
            .handle_write(RPC_COMMAND_UUID, &wifi_settings(b"X", b"y"), &mut fake())
            .expect("no exception");
        improv.answered(true);
        let urls: Vec<String> = [
            "http://192.168.1.100",
            "http://[2001:db8:1234:5678:9abc:def0:1234:5678]:8080",
            "http://[2001:db8:1234:5678:9abc:def0:1234:5679]:8080",
            "http://[2001:db8:1234:5678:9abc:def0:1234:567a]:8080",
            "http://[fe80::1234:5678:9abc:def0]:8080",
        ]
        .map(String::from)
        .to_vec();
        let pair = "http://192.168.1.100/#pair=ABCDEFGH".to_string();

        let outcome = improv
            .joined(Some(urls.clone()), Some(pair.clone()))
            .expect("built");

        assert_eq!(outcome[0], (STATUS_UUID, Payload::Bytes(vec![0x04])));
        assert_eq!(outcome[1].0, RPC_RESULT_UUID);
        let notified = single_packet(&outcome[1].1);
        assert!(
            notified.len() <= MAX_RESULT_PACKET_BYTES,
            "{}",
            notified.len()
        );
        let notified = entries(&notified);
        assert_eq!(notified[0], urls[0]);
        assert_eq!(notified[1], pair);
        assert_eq!(notified[2..], urls[1..notified.len() - 1]);
        assert!(notified.len() < urls.len() + 1, "trailing URLs dropped");

        let stored = single_packet(&improv.handle_read(RPC_RESULT_UUID));
        assert!(stored.len() <= MAX_RESULT_PACKET_BYTES);
        let stored = entries(&stored);
        assert_eq!(stored[..], urls[..stored.len()]);
        assert!(stored.iter().all(|url| !url.contains("#pair=")));
    }

    /// Without URLs there is nothing to pair against: the result stays
    /// empty, no pair URL in it.
    #[test]
    fn joined_without_urls_carries_no_pair_url() {
        let mut improv = ImprovProtocol::new(false, 250);
        let outcome = improv
            .joined(Some(Vec::new()), Some("http://x/#pair=ABCDEFGH".into()))
            .expect("built");
        assert_eq!(
            outcome[1],
            (
                RPC_RESULT_UUID,
                Payload::Packets(vec![vec![0x01, 0x00, 0x01]])
            )
        );
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

    /// The app's IDENTIFY (0x02, as the Improv BLE spec frames it) reaches
    /// the callback that notifies the Dial, and the app gets an empty
    /// result instead of the old `INVALID_RPC`.
    #[test]
    fn identify_calls_back_and_answers_with_an_empty_result() {
        let mut improv = ImprovProtocol::new(false, 250);
        let mut callbacks = fake();
        // Bare, as the spec sends it, and framed with an empty payload.
        for data in [vec![0x02], frame(0x02, &[])] {
            let answer = improv
                .handle_write(RPC_COMMAND_UUID, &data, &mut callbacks)
                .expect("no exception");
            assert_eq!(
                answer,
                Some((
                    RPC_RESULT_UUID,
                    Payload::Packets(vec![vec![0x02, 0x00, 0x02]])
                ))
            );
        }
        assert_eq!(callbacks.identified, 2);
        assert_eq!(improv.last_error, ImprovError::None);
        // A bad checksum is still refused before the callback runs.
        let mut bad = frame(0x02, &[]);
        bad[2] ^= 0xFF;
        assert_eq!(
            improv
                .handle_write(RPC_COMMAND_UUID, &bad, &mut callbacks)
                .expect("ok"),
            Some((ERROR_UUID, Payload::Bytes(vec![0x01])))
        );
        assert_eq!(callbacks.identified, 2);
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
