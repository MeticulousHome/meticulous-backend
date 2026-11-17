//! `device_identity.py`: the ESP32 owns the machine's UUID; the SoM keeps a
//! private copy (`DEVICE_UUID_CACHE_PATH`) for services that cannot ask it.

use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// `DEVICE_UUID_PATTERN`: a canonical, lower-case UUIDv4.
pub fn is_valid_device_uuid(device_uuid: &str) -> bool {
    let bytes = device_uuid.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    let hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
    bytes.iter().enumerate().all(|(i, &b)| match i {
        8 | 13 | 18 | 23 => b == b'-',
        14 => b == b'4',
        19 => matches!(b, b'8' | b'9' | b'a' | b'b'),
        _ => hex(b),
    })
}

/// `generate_device_uuid`: a UUIDv4 from the OS entropy source.
pub fn generate_device_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// `get_device_uuid_assignment`: the UUID to write to an ESP32 that has none,
/// reusing an assignment still in flight; `None` when nothing is to be done.
pub fn get_device_uuid_assignment(
    reported_uuid: &str,
    protocol_supported: bool,
    pending_assignment: Option<&str>,
) -> Option<String> {
    if is_valid_device_uuid(reported_uuid) || !protocol_supported {
        return None;
    }
    if let Some(pending) = pending_assignment.filter(|p| is_valid_device_uuid(p)) {
        return Some(pending.to_string());
    }
    Some(generate_device_uuid())
}

#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// Python's `ValueError("Invalid ESP device UUID")`.
    #[error("Invalid ESP device UUID")]
    InvalidUuid,
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// `update_device_uuid_cache`: store the ESP32's UUID (directory 0700, file
/// 0600, replaced atomically). `Ok(true)` when the cache changed.
pub fn update_device_uuid_cache(cache_path: &Path, device_uuid: &str) -> Result<bool, CacheError> {
    if !is_valid_device_uuid(device_uuid) {
        return Err(CacheError::InvalidUuid);
    }
    let cached = match std::fs::read(cache_path) {
        // Python reads it as ASCII; anything else counts as no cache.
        Ok(bytes) if bytes.is_ascii() => String::from_utf8_lossy(&bytes).trim().to_string(),
        Ok(_) => String::new(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error.into()),
    };
    let directory = cache_path.parent().unwrap_or(Path::new("."));
    if cached == device_uuid {
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(cache_path, std::fs::Permissions::from_mode(0o600))?;
        return Ok(false);
    }

    std::fs::create_dir_all(directory)?;
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
    let (mut file, temporary) = create_temporary(directory)?;
    let result = (|| -> std::io::Result<()> {
        file.write_all(format!("{device_uuid}\n").as_bytes())?;
        file.flush()?;
        file.sync_all()?;
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600))?;
        std::fs::rename(&temporary, cache_path)
    })();
    // `finally: temporary_path.unlink(missing_ok=True)`.
    let _ = std::fs::remove_file(&temporary);
    result?;
    Ok(true)
}

/// `tempfile.mkstemp(prefix=".device-uuid.", dir=directory)`.
fn create_temporary(directory: &Path) -> std::io::Result<(std::fs::File, PathBuf)> {
    let mut last_error = None;
    for _ in 0..100 {
        let suffix: String = (0..8)
            .map(|_| {
                let alphabet = b"abcdefghijklmnopqrstuvwxyz0123456789_";
                char::from(alphabet[rand::random_range(0..alphabet.len())])
            })
            .collect();
        let path = directory.join(format!(".device-uuid.{suffix}"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => return Ok((file, path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                last_error = Some(error);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| std::io::Error::other("no temporary name available")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST_UUID: &str = "123e4567-e89b-42d3-a456-426614174000";
    const SECOND_UUID: &str = "987e6543-e21b-45d3-b654-426614174999";

    fn cache_path(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join(".device-identity").join("device-uuid")
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777
    }

    /// tests/test_device_identity.py::test_validates_canonical_uuid_v4
    #[test]
    fn validates_canonical_uuid_v4() {
        assert!(is_valid_device_uuid(FIRST_UUID));
        assert!(!is_valid_device_uuid(""));
        assert!(!is_valid_device_uuid(&FIRST_UUID.to_uppercase()));
        assert!(!is_valid_device_uuid(
            "123e4567-e89b-32d3-a456-426614174000"
        ));
    }

    /// tests/test_device_identity.py::test_generates_canonical_uuid_v4
    #[test]
    fn generates_canonical_uuid_v4() {
        assert!(is_valid_device_uuid(&generate_device_uuid()));
    }

    /// tests/test_device_identity.py::test_valid_esp_uuid_never_requests_assignment
    #[test]
    fn valid_esp_uuid_never_requests_assignment() {
        assert_eq!(
            get_device_uuid_assignment(FIRST_UUID, true, Some(SECOND_UUID)),
            None
        );
    }

    /// tests/test_device_identity.py::test_unsupported_firmware_never_requests_assignment
    #[test]
    fn unsupported_firmware_never_requests_assignment() {
        assert_eq!(get_device_uuid_assignment("", false, None), None);
    }

    /// tests/test_device_identity.py::test_missing_uuid_reuses_pending_assignment_without_reading_cache
    #[test]
    fn missing_uuid_reuses_pending_assignment() {
        assert_eq!(
            get_device_uuid_assignment("", true, Some(SECOND_UUID)).as_deref(),
            Some(SECOND_UUID)
        );
    }

    /// tests/test_device_identity.py::test_missing_uuid_generates_new_assignment
    #[test]
    fn missing_uuid_generates_new_assignment() {
        let assignment = get_device_uuid_assignment("", true, None).expect("assignment");
        assert!(is_valid_device_uuid(&assignment));
    }

    /// tests/test_device_identity.py::test_invalid_pending_assignment_is_replaced
    #[test]
    fn invalid_pending_assignment_is_replaced() {
        let assignment = get_device_uuid_assignment("", true, Some("invalid")).expect("assignment");
        assert!(is_valid_device_uuid(&assignment));
    }

    /// tests/test_device_identity.py::test_creates_and_reuses_cache
    #[test]
    fn creates_and_reuses_cache() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = cache_path(&dir);
        assert!(update_device_uuid_cache(&path, FIRST_UUID).expect("update"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            format!("{FIRST_UUID}\n")
        );
        assert_eq!(mode(path.parent().expect("parent")), 0o700);
        assert_eq!(mode(&path), 0o600);

        std::fs::set_permissions(
            path.parent().expect("parent"),
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("chmod");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        assert!(!update_device_uuid_cache(&path, FIRST_UUID).expect("update"));
        assert_eq!(mode(path.parent().expect("parent")), 0o700);
        assert_eq!(mode(&path), 0o600);
    }

    /// tests/test_device_identity.py::test_esp_uuid_overwrites_different_var_som_cache
    #[test]
    fn esp_uuid_overwrites_different_cache() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = cache_path(&dir);
        std::fs::create_dir(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, format!("{FIRST_UUID}\n")).expect("write");
        assert!(update_device_uuid_cache(&path, SECOND_UUID).expect("update"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            format!("{SECOND_UUID}\n")
        );
    }

    /// tests/test_device_identity.py::test_esp_uuid_overwrites_non_ascii_var_som_cache
    #[test]
    fn esp_uuid_overwrites_non_ascii_cache() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = cache_path(&dir);
        std::fs::create_dir(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, b"\xff\xfe").expect("write");
        assert!(update_device_uuid_cache(&path, FIRST_UUID).expect("update"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            format!("{FIRST_UUID}\n")
        );
    }

    /// tests/test_device_identity.py::test_rejects_invalid_uuid_without_changing_cache
    #[test]
    fn rejects_invalid_uuid_without_changing_cache() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = cache_path(&dir);
        std::fs::create_dir(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(&path, format!("{FIRST_UUID}\n")).expect("write");
        assert!(matches!(
            update_device_uuid_cache(&path, "invalid"),
            Err(CacheError::InvalidUuid)
        ));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            format!("{FIRST_UUID}\n")
        );
        // No temporary file is left behind.
        let leftovers = std::fs::read_dir(path.parent().expect("parent"))
            .expect("dir")
            .count();
        assert_eq!(leftovers, 1);
    }
}
