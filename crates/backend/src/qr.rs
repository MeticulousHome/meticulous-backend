//! `pyqrcode.create(content).png(buffer, scale, quiet_zone, ...)` with
//! black modules on white: the QR code `/wifi/config/qr.png` serves.
//!
//! pyqrcode encodes the whole content in one mode (numeric, alphanumeric or
//! byte, ISO-8859-1), at error correction level H, in the smallest version
//! that fits; this does the same rather than the qrcode crate's mixed-mode
//! optimisation, so the symbol is the one the Python draws. The PNG is a
//! one-bit greyscale image like pypng writes for these colours.

use std::io::Write;

use qrcode::bits::Bits;
use qrcode::{Color, EcLevel, QrCode, Version};

use crate::py::PyError;

const ALPHANUMERIC: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Numeric,
    Alphanumeric,
    Binary,
}

/// `QRCode._detect_content_type`, kanji aside: no content this backend
/// encodes is all kanji.
fn detect_mode(content: &str) -> Mode {
    if !content.is_empty() && content.chars().all(|c| c.is_ascii_digit()) {
        Mode::Numeric
    } else if content.chars().all(|c| ALPHANUMERIC.contains(c)) {
        Mode::Alphanumeric
    } else {
        Mode::Binary
    }
}

/// `pyqrcode.create(content)`: the modules, row by row, `true` for dark.
pub fn create(content: &str) -> Result<(usize, Vec<bool>), PyError> {
    let mode = detect_mode(content);
    let data: Vec<u8> = match mode {
        Mode::Binary => content
            .chars()
            .map(|c| u8::try_from(u32::from(c)))
            .collect::<Result<_, _>>()
            .map_err(|_| {
                PyError::new(
                    "UnicodeEncodeError",
                    "'latin-1' codec can't encode character",
                )
            })?,
        _ => content.as_bytes().to_vec(),
    };
    for version in 1..=40 {
        let mut bits = Bits::new(Version::Normal(version));
        let pushed = match mode {
            Mode::Numeric => bits.push_numeric_data(&data),
            Mode::Alphanumeric => bits.push_alphanumeric_data(&data),
            Mode::Binary => bits.push_byte_data(&data),
        };
        if pushed.is_err() || bits.push_terminator(EcLevel::H).is_err() {
            continue;
        }
        let code =
            QrCode::with_bits(bits, EcLevel::H).map_err(|e| PyError::value_error(e.to_string()))?;
        let width = code.width();
        let modules = code
            .to_colors()
            .into_iter()
            .map(|c| c == Color::Dark)
            .collect();
        return Ok((width, modules));
    }
    Err(PyError::value_error(
        "The data will not fit in any QR code version with the given encoding and error level.",
    ))
}

/// `QRCode.png(buffer, scale, quiet_zone, module_color=black, background=white)`.
pub fn png(width: usize, modules: &[bool], scale: usize, quiet_zone: usize) -> Vec<u8> {
    let size = (width + 2 * quiet_zone) * scale;
    let row_bytes = size.div_ceil(8);
    let mut raw = Vec::with_capacity((row_bytes + 1) * size);
    for y in 0..size {
        raw.push(0); // filter type: none
        let module_y = (y / scale) as isize - quiet_zone as isize;
        let mut byte = 0u8;
        for x in 0..size {
            let module_x = (x / scale) as isize - quiet_zone as isize;
            let dark = module_y >= 0
                && module_x >= 0
                && (module_y as usize) < width
                && (module_x as usize) < width
                && modules[module_y as usize * width + module_x as usize];
            // Greyscale: white is 1, black is 0.
            if !dark {
                byte |= 0x80 >> (x % 8);
            }
            if x % 8 == 7 {
                raw.push(byte);
                byte = 0;
            }
        }
        if !size.is_multiple_of(8) {
            raw.push(byte);
        }
    }

    let mut out = Vec::new();
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(size as u32).to_be_bytes());
    ihdr.extend_from_slice(&(size as u32).to_be_bytes());
    ihdr.extend_from_slice(&[1, 0, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    // Writing into a Vec cannot fail.
    let _ = encoder.write_all(&raw);
    let compressed = encoder.finish().unwrap_or_default();
    chunk(&mut out, b"IDAT", &compressed);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc = flate2::Crc::new();
    crc.update(kind);
    crc.update(data);
    out.extend_from_slice(&crc.sum().to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_like_pyqrcode() {
        assert_eq!(detect_mode("12345"), Mode::Numeric);
        assert_eq!(detect_mode("HTTP://X:80"), Mode::Alphanumeric);
        assert_eq!(detect_mode("http://192.168.1.42:8080"), Mode::Binary);
        assert!(create("密码").is_err());
    }

    /// pyqrcode picks version 3 for a 24-byte URL at level H.
    #[test]
    fn smallest_version_at_level_h() {
        let (width, modules) = create("http://192.168.1.42:8080").expect("fits");
        assert_eq!(width, 29);
        assert_eq!(modules.len(), 29 * 29);
        // Finder pattern corner.
        assert!(modules[0]);
    }

    #[test]
    fn png_has_scaled_size_and_valid_chunks() {
        let (width, modules) = create("WIFI:S:X;T:WPA2;P:y;H:false;;").expect("fits");
        let image = png(width, &modules, 8, 2);
        assert!(image.starts_with(b"\x89PNG\r\n\x1a\n"));
        let size = u32::from_be_bytes([image[16], image[17], image[18], image[19]]) as usize;
        assert_eq!(size, (width + 4) * 8);
        assert_eq!(&image[24..29], &[1, 0, 0, 0, 0]);
        assert!(image.ends_with(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]));
    }
}
