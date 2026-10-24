//! `esp_serial/esp_tool_wrapper.py`: the firmware version shipped in
//! `UPDATE_PATH`, and flashing the ESP32-S3 (or an original ESP32 dev board)
//! over its UART.
//!
//! The Python drives `esptool` 4.6.2; this drives the `espflash` crate with
//! the same images, offsets and settings: the ROM is synced without a reset
//! (the caller has put the chip in its bootloader with the board's GPIO), the
//! flasher stub is loaded, the baud rate goes to 921600, the flash size is
//! detected (8 MB forced on the original ESP32), the bootloader header gets
//! `dio`/`80m`/that size the way esptool patches it, and each file is written
//! zlib-compressed at level 9 and verified by MD5. The chip is left in the
//! stub; the caller resets it with the GPIO afterwards. Every step is logged
//! at INFO.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use espflash::connection::{Connection, ResetAfterOperation, ResetBeforeOperation};
use espflash::flasher::{FlashSize, Flasher};
use espflash::image_format::Segment;
use espflash::target::{Chip, ProgressCallbacks};

/// `ESPToolWrapper.CONFIG_BAUD`.
pub const CONFIG_BAUD: u32 = 921_600;
const ESP_IMAGE_MAGIC: u8 = 0xE9;
const APP_DESC_MAGIC: u32 = 0xABCD_5432;
const TROUBLESHOOTING: &str = "\nFor troubleshooting steps visit: https://docs.espressif.com/projects/esptool/en/latest/troubleshooting.html";

/// `FikaSupportedESP32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FikaSupportedEsp32 {
    Esp32S1,
    Esp32S3,
}

impl FikaSupportedEsp32 {
    /// `FikaSupportedESP32.fromString`.
    pub fn from_string(chip: &str) -> Option<FikaSupportedEsp32> {
        match chip.to_uppercase().as_str() {
            "ESP32" | "ESP32S1" | "ESP32-S1" => Some(FikaSupportedEsp32::Esp32S1),
            "ESP32S3" | "ESP32-S3" => Some(FikaSupportedEsp32::Esp32S3),
            _ => None,
        }
    }

    /// esptool's `CHIP_NAME`.
    fn chip_name(self) -> &'static str {
        match self {
            FikaSupportedEsp32::Esp32S1 => "ESP32",
            FikaSupportedEsp32::Esp32S3 => "ESP32-S3",
        }
    }

    /// `IMAGE_CHIP_ID`.
    fn image_chip_id(self) -> u16 {
        match self {
            FikaSupportedEsp32::Esp32S1 => 0,
            FikaSupportedEsp32::Esp32S3 => 9,
        }
    }

    /// `BOOTLOADER_FLASH_OFFSET`.
    fn bootloader_offset(self) -> u32 {
        match self {
            FikaSupportedEsp32::Esp32S1 => 0x1000,
            FikaSupportedEsp32::Esp32S3 => 0x0,
        }
    }

    /// `ESPToolArgs.default_esp32_*_flash_mapping`.
    fn flash_mapping(self) -> [(u32, &'static str); 4] {
        [
            (self.bootloader_offset(), "bootloader.bin"),
            (0x8000, "partitions.bin"),
            (0xE000, "boot_app0.bin"),
            (0x10000, "firmware.bin"),
        ]
    }
}

/// `ESPToolWrapper.getFirmwarePath`.
pub fn firmware_path(update_path: &Path, chip: FikaSupportedEsp32) -> PathBuf {
    match chip {
        FikaSupportedEsp32::Esp32S3 => update_path.join("esp32-s3"),
        FikaSupportedEsp32::Esp32S1 => update_path.join("esp32"),
    }
}

/// An ESP32 application image (`ESP32FirmwareImage`), enough of it to read
/// the app descriptor and check what esptool checks before writing.
#[derive(Debug, Clone)]
pub struct EspImage {
    pub flash_mode: u8,
    pub flash_size_freq: u8,
    pub chip_id: u16,
    pub min_rev: u8,
    pub min_rev_full: u16,
    pub max_rev_full: u16,
    pub append_digest: bool,
    pub segments: Vec<(u32, Vec<u8>)>,
}

impl EspImage {
    /// `LoadFirmwareImage(chip, file)` (which also runs `verify()`).
    pub fn parse(bytes: &[u8], expected_chip_id: u16) -> Result<EspImage, String> {
        let mut pos = 0usize;
        fn take<'b>(bytes: &'b [u8], pos: &mut usize, n: usize) -> Result<&'b [u8], String> {
            let slice = bytes
                .get(*pos..*pos + n)
                .ok_or_else(|| format!("unpack requires a buffer of {n} bytes"))?;
            *pos += n;
            Ok(slice)
        }
        let header = take(bytes, &mut pos, 8)?;
        let (magic, segment_count, flash_mode, flash_size_freq) =
            (header[0], header[1], header[2], header[3]);
        if magic != ESP_IMAGE_MAGIC {
            return Err(format!("Invalid firmware image magic=0x{magic:x}"));
        }
        let ext = take(bytes, &mut pos, 16)?.to_vec();
        let chip_id = u16::from_le_bytes([ext[4], ext[5]]);
        if chip_id != expected_chip_id {
            tracing::info!(
                "Unexpected chip id in image. Expected {expected_chip_id} but value was {chip_id}. Is this image for a different chip model?"
            );
        }
        let append_digest = match ext[15] {
            0 => false,
            1 => true,
            other => {
                return Err(format!(
                    "Invalid value for append_digest field (0x{other:02x}). Should be 0 or 1."
                ));
            }
        };
        let mut segments = Vec::new();
        for _ in 0..segment_count {
            let head = take(bytes, &mut pos, 8)?;
            let addr = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
            let size = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
            let available = bytes.len().saturating_sub(pos).min(size);
            if available < size {
                return Err(format!(
                    "End of file reading segment 0x{addr:x}, length {size} (actual length {available})"
                ));
            }
            segments.push((addr, take(bytes, &mut pos, size)?.to_vec()));
        }
        // The checksum byte ends the next 16-byte block.
        let checksum_at = pos + (15 - pos % 16);
        if checksum_at >= bytes.len() {
            return Err("ord() expected a character, but string of length 0 found".into());
        }
        if segments.len() > 16 {
            return Err(format!(
                "Invalid segment count {} (max 16). Usually this indicates a linker script problem.",
                segments.len()
            ));
        }
        Ok(EspImage {
            flash_mode,
            flash_size_freq,
            chip_id,
            min_rev: ext[6],
            min_rev_full: u16::from_le_bytes([ext[7], ext[8]]),
            max_rev_full: u16::from_le_bytes([ext[9], ext[10]]),
            append_digest,
            segments,
        })
    }
}

/// `ESPToolWrapper.get_version_from_firmware`: the app version in the
/// ESP32-S3 firmware's app descriptor, the first 256 bytes of its (last)
/// DROM segment.
pub fn firmware_version(update_path: &Path) -> Option<String> {
    let path = update_path.join("esp32-s3").join("firmware.bin");
    let bytes = std::fs::read(&path).ok()?;
    let image = EspImage::parse(&bytes, FikaSupportedEsp32::Esp32S3.image_chip_id()).ok()?;
    let app_desc = image
        .segments
        .iter()
        .filter(|(addr, _)| (0x3C00_0000..0x3D00_0000).contains(addr))
        .map(|(_, data)| &data[..data.len().min(256)])
        .next_back()?;
    if app_desc.is_empty() {
        return None;
    }
    if app_desc.len() < 256 {
        // The Python's struct.unpack raises here; report no version instead.
        tracing::warn!("App descriptor in {} is truncated", path.display());
        return None;
    }
    let word = |at: usize| {
        u32::from_le_bytes([
            app_desc[at],
            app_desc[at + 1],
            app_desc[at + 2],
            app_desc[at + 3],
        ])
    };
    let (magic, secure_version) = (word(0), word(4));
    if magic != APP_DESC_MAGIC {
        return None;
    }
    let text = |range: std::ops::Range<usize>| std::str::from_utf8(&app_desc[range]).ok();
    let (Some(version), Some(project), Some(time), Some(date), Some(idf)) = (
        text(16..48),
        text(48..80),
        text(80..96),
        text(96..112),
        text(112..144),
    ) else {
        tracing::warn!("App descriptor in {} is not UTF-8", path.display());
        return None;
    };
    let strip = |s: &str| crate::ota::py_strip(s).to_string();
    tracing::info!("On-disk firmware file:");
    tracing::info!("Project name: {}", strip(project));
    tracing::info!("App version: {}", strip(version));
    tracing::info!("Compile time: {} {}", strip(date), strip(time));
    tracing::info!("ESP-IDF: {}", strip(idf));
    tracing::info!("Secure version: {secure_version}");
    Some(version.trim_matches([' ', '\0', '\t']).to_string())
}

/// What went wrong while flashing, as the Python reports it: `Display` is the
/// `error_msg` `Machine.startUpdate` shows in its notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlashError {
    /// An exception escaped `ESPToolWrapper.flash` (connecting, the stub,
    /// the baud rate): shown as `"{type}: {message}"`, phase "exception".
    Raised { kind: &'static str, message: String },
    /// `write_flash` failed and `flash` returned the exception: shown as
    /// `str(e)`, phase "flash".
    Returned(String),
}

impl FlashError {
    /// The phase `ESPObservability.fail_flashing` records.
    pub fn phase(&self) -> &'static str {
        match self {
            FlashError::Raised { .. } => "exception",
            FlashError::Returned(_) => "flash",
        }
    }
}

impl std::fmt::Display for FlashError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FlashError::Raised { kind, message } => write!(f, "{kind}: {message}"),
            FlashError::Returned(message) => write!(f, "{message}"),
        }
    }
}

fn fatal(message: impl Into<String>) -> FlashError {
    FlashError::Raised {
        kind: "FatalError",
        message: message.into(),
    }
}

/// esptool's `FLASH_SIZES` for the ESP32 family (bits 7..4 of header byte 3).
fn flash_size_code(size: &str) -> Result<u8, String> {
    Ok(match size {
        "1MB" => 0x00,
        "2MB" => 0x10,
        "4MB" => 0x20,
        "8MB" => 0x30,
        "16MB" => 0x40,
        "32MB" => 0x50,
        "64MB" => 0x60,
        "128MB" => 0x70,
        other => {
            return Err(format!(
                "Flash size '{other}' is not supported by this chip type. Supported sizes: 1MB, 2MB, 4MB, 8MB, 16MB, 32MB, 64MB, 128MB"
            ));
        }
    })
}

/// `flash_size_bytes`.
fn flash_size_bytes(size: &str) -> u64 {
    let (number, unit) = if let Some(n) = size.strip_suffix("MB") {
        (n, 1024 * 1024)
    } else if let Some(n) = size.strip_suffix("KB") {
        (n, 1024)
    } else {
        (size, 1)
    };
    number.parse::<u64>().unwrap_or(0) * unit
}

fn flash_size_name(size: FlashSize) -> &'static str {
    match size {
        FlashSize::_256Kb => "256KB",
        FlashSize::_512Kb => "512KB",
        FlashSize::_1Mb => "1MB",
        FlashSize::_2Mb => "2MB",
        FlashSize::_4Mb => "4MB",
        FlashSize::_8Mb => "8MB",
        FlashSize::_16Mb => "16MB",
        FlashSize::_32Mb => "32MB",
        FlashSize::_64Mb => "64MB",
        FlashSize::_128Mb => "128MB",
        FlashSize::_256Mb => "256MB",
        _ => "4MB",
    }
}

/// esptool 4.6.2's `_update_image_flash_params` with `--flash_mode dio
/// --flash_freq 80m --flash_size <size>`: only the image at the bootloader
/// offset, only a valid image, and not when a SHA-256 digest protects it.
pub fn update_image_flash_params(
    chip: FikaSupportedEsp32,
    address: u32,
    flash_size: &str,
    image: Vec<u8>,
) -> Result<Vec<u8>, String> {
    if image.len() < 8 || address != chip.bootloader_offset() {
        return Ok(image);
    }
    if image[0] != ESP_IMAGE_MAGIC {
        tracing::info!(
            "Warning: Image file at 0x{address:x} doesn't look like an image file, so not changing any flash settings."
        );
        return Ok(image);
    }
    if EspImage::parse(&image, chip.image_chip_id()).is_err() {
        tracing::info!(
            "Warning: Image file at 0x{address:x} is not a valid {} image, so not changing any flash settings.",
            chip.chip_name()
        );
        return Ok(image);
    }
    let sha_implies_keep = image.get(8 + 15) == Some(&1);
    let (mut flash_mode, size_freq) = (image[2], image[3]);
    let (mut flash_freq, mut size) = (size_freq & 0x0F, size_freq & 0xF0);
    let keep_warning = |arg: &str, value: &str| {
        tracing::info!(
            "Warning: Image file at {address:#x} is protected with a hash checksum, so not changing the flash {arg} setting. Use the --flash_{arg}=keep option instead of --flash_{arg}={value} in order to remove this warning, or use the --dont-append-digest option for the elf2image command in order to generate an image file without a hash checksum"
        );
    };
    const DIO: u8 = 2;
    const FREQ_80M: u8 = 0xF;
    if flash_mode != DIO && sha_implies_keep {
        keep_warning("mode", "dio");
    } else {
        flash_mode = DIO;
    }
    if flash_freq != FREQ_80M && sha_implies_keep {
        keep_warning("frequency", "80m");
    } else {
        flash_freq = FREQ_80M;
    }
    let new_size = flash_size_code(flash_size)?;
    if size != new_size && sha_implies_keep {
        keep_warning("size", flash_size);
    } else {
        size = new_size;
    }
    let params = [flash_mode, size + flash_freq];
    if params != image[2..4] {
        tracing::info!("Flash params set to 0x{:04x}", u16::from_be_bytes(params));
        let mut image = image;
        image[2..4].copy_from_slice(&params);
        return Ok(image);
    }
    Ok(image)
}

/// `pad_to(data, 4)` with 0xFF.
fn pad_to_4(mut data: Vec<u8>) -> Vec<u8> {
    let rem = data.len() % 4;
    if rem != 0 {
        data.extend(std::iter::repeat_n(0xFF, 4 - rem));
    }
    data
}

/// esptool's `Writing at 0x... (n %)` lines, every tenth of a file.
struct LogProgress {
    addr: u32,
    total: usize,
    last_decile: usize,
}

impl ProgressCallbacks for LogProgress {
    fn init(&mut self, addr: u32, total: usize) {
        self.addr = addr;
        self.total = total.max(1);
        self.last_decile = 0;
        tracing::info!("Writing at 0x{addr:08x}... (0 %)");
    }

    fn update(&mut self, current: usize) {
        let percent = 100 * current / self.total;
        if percent / 10 > self.last_decile {
            self.last_decile = percent / 10;
            tracing::info!("Writing at 0x{:08x}... ({percent} %)", self.addr);
        }
    }

    fn verifying(&mut self) {
        tracing::info!("Verifying 0x{:08x}...", self.addr);
    }

    fn finish(&mut self, _skipped: bool) {
        tracing::info!("Hash of data verified.");
    }
}

fn describe(error: &espflash::Error) -> String {
    match error {
        espflash::Error::VerifyFailed => "MD5 of file does not match data in flash!".into(),
        other => other.to_string(),
    }
}

/// The files of the flash mapping, with their path (`argfile.name`).
type LoadedFile = (u32, PathBuf, Vec<u8>);

/// `ESPToolArgs.loadFlashMapping`. PARITY: a missing file leaves the
/// Python with nothing to write, and `write_flash` then fails on the empty
/// list with `IndexError: list index out of range`; nothing is written.
fn load_flash_mapping(
    update_path: &Path,
    chip: FikaSupportedEsp32,
) -> Result<Vec<LoadedFile>, FlashError> {
    let dir = firmware_path(update_path, chip);
    let mut files = Vec::new();
    for (address, name) in chip.flash_mapping() {
        let path = dir.join(name);
        match std::fs::read(&path) {
            Ok(bytes) => files.push((address, path, bytes)),
            Err(error) => {
                tracing::error!(
                    "Failed to load firmware from disk: [Errno {}] {}: '{}'",
                    error.raw_os_error().unwrap_or(0),
                    error,
                    path.display()
                );
                tracing::error!("Failed to flash: IndexError");
                return Err(FlashError::Returned("list index out of range".into()));
            }
        }
    }
    Ok(files)
}

/// Flash the firmware in `update_path` over `port`, which must be open at
/// 115200 baud with the chip already in its ROM bootloader. The port comes
/// back at its original baud rate whatever happens; reset the chip with the
/// GPIO afterwards (the Python's `SerialConnection.sendUpdate`).
pub fn flash(
    port: serialport::TTYPort,
    update_path: &Path,
) -> (serialport::TTYPort, Result<(), FlashError>) {
    use serialport::SerialPort as _;
    let initial_baud = port.baud_rate().unwrap_or(115_200);
    tracing::info!("Calling into espflash");
    let connection = Connection::new(
        port,
        serialport::UsbPortInfo {
            vid: 0,
            pid: 0,
            serial_number: None,
            manufacturer: None,
            product: None,
        },
        ResetAfterOperation::NoResetNoStub,
        ResetBeforeOperation::NoReset,
        initial_baud,
    );
    tracing::info!("Connecting to the ROM bootloader (no reset)");
    let mut flasher =
        match Flasher::try_connect(connection, true, true, false, None, Some(CONFIG_BAUD)) {
            Ok(flasher) => flasher,
            Err(failure) => {
                let (error, connection) = *failure;
                let message = match &error {
                    espflash::Error::Connection(_) => {
                        format!("Failed to connect to ESP32-S3: {error}{TROUBLESHOOTING}")
                    }
                    other => describe(other),
                };
                tracing::error!("Flashing failed before writing: {message}");
                return (
                    restore_baud(connection.into_serial(), initial_baud),
                    Err(fatal(message)),
                );
            }
        };
    let result = flash_connected(&mut flasher, update_path);
    tracing::info!("Done, Resetting baudrate");
    let port = flasher.into_connection().into_serial();
    (restore_baud(port, initial_baud), result)
}

fn restore_baud(mut port: serialport::TTYPort, baud: u32) -> serialport::TTYPort {
    use serialport::SerialPort as _;
    if let Err(error) = port.set_baud_rate(baud) {
        tracing::warn!("Could not restore the baud rate to {baud}: {error}");
    }
    port
}

fn flash_connected(flasher: &mut Flasher, update_path: &Path) -> Result<(), FlashError> {
    tracing::info!("Detecting chip type...");
    let chip = match flasher.chip() {
        Chip::Esp32s3 => FikaSupportedEsp32::Esp32S3,
        Chip::Esp32 => {
            tracing::info!("Failed to get ESP32 ChipID, assuming ESP32-S1");
            FikaSupportedEsp32::Esp32S1
        }
        other => {
            // The Python carries on as an ESP32-S3; writing an ESP32-S3
            // image to another chip cannot work, so stop here.
            tracing::info!("unexpected chip id");
            return Err(fatal(format!("Unsupported chip {other}, not flashing")));
        }
    };
    let info = flasher.device_info().map_err(|e| fatal(describe(&e)))?;
    let (major, minor) = info.revision.unwrap_or((0, 0));
    tracing::info!("Chip is {} (revision v{major}.{minor})", chip.chip_name());
    tracing::info!("Features: {}", info.features.join(", "));
    if let Some(mac) = &info.mac_address {
        tracing::info!("MAC: {mac}");
    }
    tracing::info!("Stub running, baud rate {CONFIG_BAUD}");

    // We compile the ESP32 image for 8 MB, so we flash for that as well.
    let flash_size = match chip {
        FikaSupportedEsp32::Esp32S1 => "8MB",
        FikaSupportedEsp32::Esp32S3 => flash_size_name(info.flash_size),
    };
    tracing::info!("flash_size = {flash_size}");

    let files = load_flash_mapping(update_path, chip)?;
    let rev_full = major * 100 + minor;
    check_before_writing(flasher, chip, &files, flash_size, rev_full, major)
        .inspect_err(|e| tracing::error!("Failed to flash: FatalError ({e})"))?;

    let mut segments = Vec::new();
    for (address, path, bytes) in files {
        let image = pad_to_4(bytes);
        if image.is_empty() {
            tracing::info!("WARNING: File {} is empty", path.display());
            continue;
        }
        let image = update_image_flash_params(chip, address, flash_size, image)
            .map_err(|e| FlashError::Returned(e.clone()))?;
        tracing::info!(
            "Flash will be erased from 0x{:08x} to 0x{:08x}...",
            address,
            (u64::from(address) + image.len() as u64).div_ceil(0x1000) * 0x1000 - 1
        );
        tracing::info!(
            "{} ({} bytes at 0x{address:08x}, md5 {})",
            path.display(),
            image.len(),
            hex::encode(<md5::Md5 as md5::Digest>::digest(&image))
        );
        segments.push(Segment {
            addr: address,
            data: Cow::Owned(image),
        });
    }
    let mut progress = LogProgress {
        addr: 0,
        total: 1,
        last_decile: 0,
    };
    match flasher.write_bins_to_flash(&segments, &mut progress) {
        Ok(()) => {
            tracing::info!("Leaving...");
            Ok(())
        }
        Err(error) => {
            tracing::error!("Failed to flash: {error:?}");
            Err(FlashError::Returned(describe(&error)))
        }
    }
}

/// What esptool's `write_flash` checks before touching the flash.
fn check_before_writing(
    flasher: &mut Flasher,
    chip: FikaSupportedEsp32,
    files: &[LoadedFile],
    flash_size: &str,
    rev_full: u32,
    major: u32,
) -> Result<(), FlashError> {
    let returned = |message: String| FlashError::Returned(message);
    if chip == FikaSupportedEsp32::Esp32S3 && !flasher.secure_download_mode() {
        // ESP32S3ROM's eFuse readings.
        const EFUSE_BASE: u32 = 0x6000_7000;
        let connection = flasher.connection();
        let read = |connection: &mut Connection, offset: u32| {
            connection
                .read_reg(EFUSE_BASE + offset)
                .map_err(|e| FlashError::Returned(describe(&e)))
        };
        let secure_boot = read(connection, 0x038)? & (1 << 20) != 0;
        if secure_boot && files.iter().any(|(address, _, _)| *address < 0x8000) {
            return Err(returned(
                "Secure Boot detected, writing to flash regions < 0x8000 is disabled to protect the bootloader. Use --force to override, please use with caution, otherwise it may brick your device!".into(),
            ));
        }
        let manual_encrypt_disabled = read(connection, 0x030)? & (1 << 20) != 0;
        let crypt_cnt = (read(connection, 0x034)? >> 18) & 0x7;
        if manual_encrypt_disabled && crypt_cnt.count_ones() % 2 == 1 {
            return Err(returned(
                "WARNING: Detected flash encryption enabled and download manual encrypt disabled.\nFlashing plaintext binary may brick your device! Use --force to override the warning.".into(),
            ));
        }
    }
    for (_, path, bytes) in files {
        let Ok(image) = EspImage::parse(bytes, chip.image_chip_id()) else {
            continue;
        };
        if image.chip_id != chip.image_chip_id() {
            return Err(returned(format!(
                "{} is not an {} image. Use --force to flash anyway.",
                path.display(),
                chip.chip_name()
            )));
        }
        let use_rev_full = match image.max_rev_full {
            0 => false,
            65535 => !(image.min_rev_full == 0 && image.min_rev != 0),
            _ => true,
        };
        if use_rev_full {
            let max = u32::from(image.max_rev_full);
            let min = u32::from(image.min_rev_full);
            if rev_full < min || rev_full > max {
                let upper = if max == 65535 {
                    "max rev not set] ".to_string()
                } else {
                    format!("v{}.{}] ", max / 100, max % 100)
                };
                return Err(returned(format!(
                    "{} requires chip revision in range [v{}.{} - {upper}(this chip is revision v{}.{}). Use --force to flash anyway.",
                    path.display(),
                    min / 100,
                    min % 100,
                    rev_full / 100,
                    rev_full % 100
                )));
            }
        } else if major < u32::from(image.min_rev) {
            return Err(returned(format!(
                "{} requires chip revision {} or higher (this chip is revision {major}). Use --force to flash anyway.",
                path.display(),
                image.min_rev
            )));
        }
    }
    let flash_end = flash_size_bytes(flash_size);
    for (address, path, bytes) in files {
        if u64::from(*address) + bytes.len() as u64 > flash_end {
            return Err(returned(format!(
                "File {} (length {}) at offset {address} will not fit in {flash_end} bytes of flash. Use --flash_size argument, or change flashing address.",
                path.display(),
                bytes.len()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal ESP32-S3 image: header, extended header, segments, checksum.
    fn image(chip_id: u16, append_digest: u8, segments: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut out = vec![
            ESP_IMAGE_MAGIC,
            segments.len() as u8,
            0x00,
            0x20,
            0,
            0,
            0,
            0,
        ];
        let mut ext = vec![0xEE, 0, 0, 0];
        ext.extend_from_slice(&chip_id.to_le_bytes());
        ext.push(0);
        ext.extend_from_slice(&0u16.to_le_bytes());
        ext.extend_from_slice(&0xFFFFu16.to_le_bytes());
        ext.extend_from_slice(&[0, 0, 0, 0]);
        ext.push(append_digest);
        out.extend(ext);
        for (addr, data) in segments {
            out.extend_from_slice(&addr.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(data);
        }
        while out.len() % 16 != 15 {
            out.push(0);
        }
        out.push(0xEF);
        out
    }

    fn app_desc(version: &str) -> Vec<u8> {
        let mut desc = Vec::new();
        desc.extend_from_slice(&APP_DESC_MAGIC.to_le_bytes());
        desc.extend_from_slice(&7u32.to_le_bytes());
        desc.extend_from_slice(&[0; 8]);
        let field = |s: &str, n: usize| {
            let mut f = s.as_bytes().to_vec();
            f.resize(n, 0);
            f
        };
        desc.extend(field(version, 32));
        desc.extend(field("EspressoFirmware", 32));
        desc.extend(field("12:00:00", 16));
        desc.extend(field("Oct 24 2026", 16));
        desc.extend(field("v5.1", 32));
        desc.extend(vec![0; 32 + 80]);
        desc.extend(vec![0xAB; 64]);
        desc
    }

    /// Machine.refreshAvailableFirmware: the version in the shipped
    /// firmware.bin, from its DROM app descriptor.
    #[test]
    fn firmware_version_from_the_app_descriptor() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(firmware_version(dir.path()), None);
        let fw = dir.path().join("esp32-s3");
        std::fs::create_dir_all(&fw).expect("dir");
        let bytes = image(
            9,
            0,
            &[
                (0x3FC8_8000, vec![1; 16]),
                (0x3C00_0020, app_desc("1.4.2-beta")),
                (0x4200_0020, vec![2; 32]),
            ],
        );
        std::fs::write(fw.join("firmware.bin"), &bytes).expect("fw");
        assert_eq!(firmware_version(dir.path()).as_deref(), Some("1.4.2-beta"));

        std::fs::write(
            fw.join("firmware.bin"),
            image(9, 0, &[(0x3C00_0020, vec![0; 300])]),
        )
        .expect("fw");
        assert_eq!(firmware_version(dir.path()), None, "wrong magic");
        std::fs::write(fw.join("firmware.bin"), b"\x00garbage").expect("fw");
        assert_eq!(firmware_version(dir.path()), None);
        std::fs::write(fw.join("firmware.bin"), &bytes[..bytes.len() - 40]).expect("fw");
        assert_eq!(firmware_version(dir.path()), None, "truncated");
    }

    #[test]
    fn chip_names_and_paths() {
        assert_eq!(
            FikaSupportedEsp32::from_string("esp32-s3"),
            Some(FikaSupportedEsp32::Esp32S3)
        );
        assert_eq!(
            FikaSupportedEsp32::from_string("ESP32"),
            Some(FikaSupportedEsp32::Esp32S1)
        );
        assert_eq!(FikaSupportedEsp32::from_string("esp8266"), None);
        assert_eq!(
            firmware_path(Path::new("/opt/fw"), FikaSupportedEsp32::Esp32S3),
            PathBuf::from("/opt/fw/esp32-s3")
        );
        assert_eq!(
            FikaSupportedEsp32::Esp32S3.flash_mapping(),
            [
                (0x0, "bootloader.bin"),
                (0x8000, "partitions.bin"),
                (0xE000, "boot_app0.bin"),
                (0x10000, "firmware.bin"),
            ]
        );
        assert_eq!(
            FikaSupportedEsp32::Esp32S1.flash_mapping()[0],
            (0x1000, "bootloader.bin")
        );
    }

    /// esptool patches the bootloader header to dio/80m/detected size, and
    /// only the bootloader, only valid images, never digest-protected ones.
    #[test]
    fn bootloader_header_patching() {
        let s3 = FikaSupportedEsp32::Esp32S3;
        let boot = image(9, 0, &[(0x403C_8000, vec![0; 16])]);
        let patched = update_image_flash_params(s3, 0x0, "16MB", boot.clone()).expect("patched");
        assert_eq!(&patched[2..4], &[0x02, 0x4F]);
        assert_eq!(&patched[4..], &boot[4..]);
        assert_eq!(
            update_image_flash_params(s3, 0x8000, "16MB", boot.clone()),
            Ok(boot.clone())
        );
        let protected = image(9, 1, &[(0x403C_8000, vec![0; 16])]);
        assert_eq!(
            update_image_flash_params(s3, 0x0, "16MB", protected.clone()),
            Ok(protected)
        );
        let not_image = vec![0xAA; 64];
        assert_eq!(
            update_image_flash_params(s3, 0x0, "8MB", not_image.clone()),
            Ok(not_image)
        );
        let mut truncated = boot.clone();
        truncated.truncate(30);
        assert_eq!(
            update_image_flash_params(s3, 0x0, "8MB", truncated.clone()),
            Ok(truncated)
        );
        assert!(update_image_flash_params(s3, 0x0, "256KB", boot).is_err());
        let s1_boot = image(0, 0, &[(0x4008_0000, vec![0; 8])]);
        let patched =
            update_image_flash_params(FikaSupportedEsp32::Esp32S1, 0x1000, "8MB", s1_boot)
                .expect("patched");
        assert_eq!(&patched[2..4], &[0x02, 0x3F]);
    }

    /// A missing firmware file is the Python's IndexError, written as is.
    #[test]
    fn missing_firmware_files_fail_like_the_python() {
        let dir = tempfile::tempdir().expect("tempdir");
        let error =
            load_flash_mapping(dir.path(), FikaSupportedEsp32::Esp32S3).expect_err("missing");
        assert_eq!(
            error,
            FlashError::Returned("list index out of range".into())
        );
        assert_eq!(error.to_string(), "list index out of range");
        assert_eq!(error.phase(), "flash");
        let raised = fatal("Failed to connect to ESP32-S3: x");
        assert_eq!(
            raised.to_string(),
            "FatalError: Failed to connect to ESP32-S3: x"
        );
        assert_eq!(raised.phase(), "exception");
    }

    #[test]
    fn image_parsing_errors() {
        assert!(EspImage::parse(&[0xE9, 1, 0, 0], 9).is_err());
        let mut bad_digest = image(9, 0, &[]);
        bad_digest[8 + 15] = 7;
        assert!(EspImage::parse(&bad_digest, 9).is_err());
        let mut many = image(9, 0, &[]);
        many[1] = 17;
        assert!(EspImage::parse(&many, 9).is_err());
        assert_eq!(
            pad_to_4(vec![1, 2, 3, 4, 5]),
            vec![1, 2, 3, 4, 5, 0xFF, 0xFF, 0xFF]
        );
        assert_eq!(flash_size_bytes("8MB"), 8 * 1024 * 1024);
    }
}
