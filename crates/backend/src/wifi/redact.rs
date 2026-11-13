//! `wifi.redact_ssid`: network names in logs become the same per-device
//! token the emit-time redaction filter and the watcher produce
//! (`log_redactor.pseudonym("SSID", ssid, key)`), so a backend line and a
//! NetworkManager line about the same network can be matched up.

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

use parking_lot::Mutex;
use sha2::{Digest, Sha256};

const KEY_SIZE: usize = 32;

pub struct SsidRedactor {
    path: PathBuf,
    key: Mutex<Option<Vec<u8>>>,
}

impl SsidRedactor {
    pub fn new(path: PathBuf) -> SsidRedactor {
        SsidRedactor {
            path,
            key: Mutex::new(None),
        }
    }

    /// `redact_ssid(ssid)`: stars when the key cannot be read, never the name.
    pub fn redact(&self, ssid: &str) -> String {
        if ssid.is_empty() {
            return String::new();
        }
        match self.cached_key() {
            Some(key) => pseudonym("SSID", ssid, &key),
            None => "*".repeat(ssid.chars().count()),
        }
    }

    /// `load_cached_key()`: only a successful load is cached.
    fn cached_key(&self) -> Option<Vec<u8>> {
        let mut cached = self.key.lock();
        if cached.is_none() {
            *cached = load_key(&self.path).ok();
        }
        cached.clone()
    }
}

/// `log_redactor.pseudonym(kind, value, key)`.
pub fn pseudonym(kind: &str, value: &str, key: &[u8]) -> String {
    let digest = hmac_sha256(key, format!("{kind}\0{value}").as_bytes());
    format!("[{kind}_{}]", &hex::encode(digest)[..8])
}

/// `log_redactor.load_key(path)`: read the per-device key, creating it on
/// first use.
fn load_key(path: &PathBuf) -> std::io::Result<Vec<u8>> {
    match read_key(path) {
        Ok(key) => return Ok(key),
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
        Err(_) => {}
    }
    let key: [u8; KEY_SIZE] = rand::random();
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path);
    match file {
        Ok(mut file) => {
            file.write_all(&key)?;
            file.flush()?;
            file.sync_all()?;
            Ok(key.to_vec())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // Another process won the first-use race; let it finish writing.
            std::thread::sleep(std::time::Duration::from_millis(100));
            read_key(path)
        }
        Err(error) => Err(error),
    }
}

fn read_key(path: &PathBuf) -> std::io::Result<Vec<u8>> {
    let key = std::fs::read(path)?;
    if key.len() != KEY_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "redaction key at '{}' must be exactly {KEY_SIZE} bytes",
                path.display()
            ),
        ));
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(key)
}

/// HMAC-SHA256 (RFC 2104).
fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut padded = [0u8; BLOCK];
    if key.len() > BLOCK {
        padded[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        padded[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(padded.map(|b| b ^ 0x36));
    inner.update(message);
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(padded.map(|b| b ^ 0x5c));
    outer.update(inner);
    outer.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_matches_rfc_4231() {
        let digest = hmac_sha256(&[0x0b; 20], b"Hi There");
        assert_eq!(
            hex::encode(digest),
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    /// Token from `log_redactor.pseudonym("SSID", "HomeNet", b"k" * 32)`.
    #[test]
    fn pseudonym_matches_log_redactor() {
        assert_eq!(pseudonym("SSID", "HomeNet", &[b'k'; 32]), "[SSID_ac2b575a]");
    }

    #[test]
    fn missing_key_directory_falls_back_to_stars() {
        let dir = tempfile::tempdir().expect("tempdir");
        let redactor = SsidRedactor::new(dir.path().join("missing/key"));
        assert_eq!(redactor.redact("HomeNet"), "*******");
        assert_eq!(redactor.redact(""), "");
        let created = SsidRedactor::new(dir.path().join("key"));
        let token = created.redact("HomeNet");
        assert!(token.starts_with("[SSID_") && token.ends_with(']'));
        assert_eq!(
            std::fs::read(dir.path().join("key")).expect("key").len(),
            32
        );
        assert_eq!(created.redact("HomeNet"), token);
    }
}
