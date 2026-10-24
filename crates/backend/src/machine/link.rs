//! The serial link to the ESP32 (`esp_serial/connection/*.py`).
//!
//! On a machine the ESP32 hangs off `/dev/ttymxc0` and four GPIO lines of
//! the board reset it and select its bootloader. A dev board on USB resets
//! through DTR/RTS like esptool does. The emulator and the test harnesses
//! hand the backend a pty instead.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use serialport::SerialPort;

pub const BAUDRATE: u32 = 115_200;

/// How long one read waits for bytes before the line reader checks its own
/// deadline (pyserial's `timeout`).
const READ_TIMEOUT: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkKind {
    /// The machine's UART and the board's GPIO lines (`FikaSerialConnection`).
    Fika,
    /// An ESP32 dev board (`USBSerialConnection`).
    Usb,
    /// A serial device the harness or fika-sil provides, no reset lines.
    Device(PathBuf),
    /// The emulator's pty.
    Emulator(PathBuf),
}

/// The board's lines (Fika V3+ pin configuration): (chip, line, alias).
const FIKA_PINS: [(u32, u32, &str); 4] = [(4, 9, "en"), (0, 7, "esp_en"), (0, 8, "io0"), (3, 26, "buffer_pin")];

struct FikaPins {
    requests: Vec<(u32, gpiocdev::Request)>,
}

impl FikaPins {
    /// `_requestPins`: every line an output at 0, like a gpiod v1 request
    /// without default values.
    fn request() -> Result<FikaPins, String> {
        let mut requests = Vec::new();
        for chip in [0u32, 3, 4] {
            let lines: Vec<u32> = FIKA_PINS.iter().filter(|(c, _, _)| *c == chip).map(|(_, l, _)| *l).collect();
            let request = gpiocdev::Request::builder()
                .on_chip(format!("/dev/gpiochip{chip}"))
                .with_consumer("fika_serial_connection")
                .with_lines(&lines)
                .as_output(gpiocdev::line::Value::Inactive)
                .request()
                .map_err(|e| format!("gpiochip{chip}: {e}"))?;
            requests.push((chip, request));
        }
        Ok(FikaPins { requests })
    }

    fn set(&self, alias: &str, high: bool) {
        tracing::debug!("Setting pin {alias} to {}", u8::from(high));
        let Some((chip, line, _)) = FIKA_PINS.iter().find(|(_, _, a)| *a == alias) else { return };
        if let Some((_, request)) = self.requests.iter().find(|(c, _)| c == chip) {
            let value = if high { gpiocdev::line::Value::Active } else { gpiocdev::line::Value::Inactive };
            if let Err(error) = request.set_value(*line, value) {
                tracing::warn!("Could not set pin {alias}: {error}");
            }
        }
    }
}

pub struct EspLink {
    pub kind: LinkKind,
    pub device: String,
    port: Box<dyn SerialPort>,
    pins: Option<FikaPins>,
}

impl EspLink {
    /// `SerialConnection.__init__`: open the port, or fail like the Python's
    /// "Couldn't connect to ESP32".
    pub fn open(kind: LinkKind) -> Result<EspLink, String> {
        let (device, pins) = match &kind {
            LinkKind::Fika => {
                tracing::info!("Connecting to Fika ESP32");
                ("/dev/ttymxc0".to_string(), Some(FikaPins::request()?))
            }
            LinkKind::Usb => (find_usb_device().ok_or("No serial Device found!")?, None),
            LinkKind::Device(path) | LinkKind::Emulator(path) => (path.display().to_string(), None),
        };
        tracing::info!("Testing connect to {device}");
        let port = serialport::new(&device, BAUDRATE)
            .timeout(READ_TIMEOUT)
            .open()
            .map_err(|e| {
                tracing::info!("Serial Exception raised while trying to connect: {e}");
                "Couldn't connect to ESP32".to_string()
            })?;
        tracing::info!("Successfully connected to {device}");
        tracing::info!("An ESP32 is connected");
        Ok(EspLink { kind, device, port, pins })
    }

    /// A second handle on the same port for the reader thread.
    pub fn reader(&self) -> Result<Box<dyn SerialPort>, String> {
        self.port.try_clone().map_err(|e| e.to_string())
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.port.write_all(bytes)?;
        self.port.flush()
    }

    pub fn reset_input_buffer(&mut self) {
        let _ = self.port.clear(serialport::ClearBuffer::Input);
    }

    /// The board-specific reset, optionally into the ROM bootloader.
    pub fn reset(&mut self, bootloader: bool) {
        tracing::info!("Resetting ESP32, bootloder = {bootloader}");
        match (&self.kind, &self.pins) {
            (LinkKind::Fika, Some(pins)) => {
                pins.set("io0", false);
                // Hold the chip in reset.
                pins.set("esp_en", true);
                std::thread::sleep(Duration::from_millis(100));
                pins.set("io0", bootloader);
                pins.set("esp_en", false);
                if bootloader {
                    std::thread::sleep(Duration::from_millis(100));
                    pins.set("io0", false);
                }
            }
            (LinkKind::Usb, _) => {
                // esptool's ClassicReset for the bootloader, HardReset otherwise.
                if bootloader {
                    let _ = self.port.write_data_terminal_ready(false);
                    let _ = self.port.write_request_to_send(true);
                    std::thread::sleep(Duration::from_millis(100));
                    let _ = self.port.write_data_terminal_ready(true);
                    let _ = self.port.write_request_to_send(false);
                    std::thread::sleep(Duration::from_millis(50));
                    let _ = self.port.write_data_terminal_ready(false);
                } else {
                    let _ = self.port.write_request_to_send(true);
                    std::thread::sleep(Duration::from_millis(100));
                    let _ = self.port.write_request_to_send(false);
                }
            }
            _ => {}
        }
    }

    pub fn set_baud_rate(&mut self, baud: u32) {
        let _ = self.port.set_baud_rate(baud);
    }

    /// The port itself, for the flasher.
    pub fn port_mut(&mut self) -> &mut Box<dyn SerialPort> {
        &mut self.port
    }
}

/// `USBSerialConnection.connect()`: the first `ttyUSB*` or `ttyS*` device.
fn find_usb_device() -> Option<String> {
    let mut devices: Vec<String> = std::fs::read_dir("/dev")
        .ok()?
        .filter_map(Result::ok)
        .map(|e| format!("/dev/{}", e.file_name().to_string_lossy()))
        .filter(|p| p.contains("ttyUSB") || p.contains("ttyS"))
        .collect();
    devices.sort();
    devices.into_iter().next()
}

/// `Machine.ReadLine`: lines out of a byte stream, with a deadline.
pub struct LineReader {
    port: Box<dyn SerialPort>,
    buf: Vec<u8>,
}

impl LineReader {
    pub fn new(port: Box<dyn SerialPort>) -> LineReader {
        LineReader { port, buf: Vec::new() }
    }

    /// One line including its `\n`, or `None` when none completed before
    /// `timeout`.
    pub fn read_line(&mut self, timeout: Duration) -> Option<Vec<u8>> {
        let started = std::time::Instant::now();
        loop {
            if let Some(i) = self.buf.iter().position(|b| *b == b'\n') {
                let rest = self.buf.split_off(i + 1);
                return Some(std::mem::replace(&mut self.buf, rest));
            }
            let mut chunk = [0u8; 2048];
            match self.port.read(&mut chunk) {
                Ok(0) => {}
                Ok(n) => {
                    self.buf.extend_from_slice(&chunk[..n]);
                    continue;
                }
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => {
                    tracing::debug!("serial read failed: {e}");
                    std::thread::sleep(READ_TIMEOUT);
                }
            }
            if started.elapsed() >= timeout {
                return None;
            }
        }
    }
}
