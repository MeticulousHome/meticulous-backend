//! Log connectivity failures without their credentials: a port of
//! `sensitive_logging.py`. Each helper describes a value by its size or type,
//! never by its content.

/// `credential_metadata(ssid, password)`: UTF-8 byte lengths only.
pub fn credential_metadata(ssid: &str, password: &str) -> String {
    format!(
        "ssid_bytes={}, password_bytes={}",
        ssid.len(),
        password.len()
    )
}

/// `payload_metadata(payload)` for one buffer: `"<n> bytes"`.
pub fn payload_metadata(payload: &[u8]) -> String {
    format!("{} bytes", payload.len())
}

/// `payload_metadata(payload)` for a list of buffers: their total size.
pub fn payload_parts_metadata<P: AsRef<[u8]>>(parts: &[P]) -> String {
    let size: usize = parts.iter().map(|part| part.as_ref().len()).sum();
    format!("{size} bytes")
}

/// `exception_metadata(error)`: the error's type name, never its message.
///
/// Python logs the exception class (`RuntimeError`); this logs the Rust type's
/// name without its module path or generic arguments. Log wording is not a
/// contract; what matters is that no message text gets through.
pub fn exception_metadata<E: ?Sized>(_error: &E) -> String {
    let full = std::any::type_name::<E>();
    let base = full.split('<').next().unwrap_or(full);
    base.rsplit("::").next().unwrap_or(base).to_owned()
}

/// `command_metadata(command)`: the executable's base name and the argument
/// count, never the arguments (an `nmcli` call carries the PSK).
pub fn command_metadata<S: AsRef<str>>(command: &[S]) -> String {
    let Some(first) = command.first() else {
        return "<empty command>".to_owned();
    };
    let first = first.as_ref();
    // os.path.basename: everything after the last '/'.
    let executable = first.rsplit('/').next().unwrap_or(first);
    format!("{executable} ({} args)", command.len() - 1)
}
