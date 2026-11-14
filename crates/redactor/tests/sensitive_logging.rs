//! Port of `tests/test_sensitive_logging.py`.

use redactor::{
    command_metadata, credential_metadata, exception_metadata, payload_metadata,
    payload_parts_metadata,
};

/// `test_credential_metadata_never_contains_unicode_credentials`.
#[test]
fn credential_metadata_never_contains_unicode_credentials() {
    let ssid = "Private-\u{7F51}\u{7EDC}";
    let password = "SENTINEL-\u{5BC6}\u{7801}-p\u{E4}ssword";

    let metadata = credential_metadata(ssid, password);

    assert!(!metadata.contains(ssid));
    assert!(!metadata.contains(password));
    assert_eq!(metadata, "ssid_bytes=14, password_bytes=25");
}

/// `test_payload_metadata_never_contains_raw_packet_bytes`.
#[test]
fn payload_metadata_never_contains_raw_packet_bytes() {
    let payload = b"SENTINEL-RAW-BLE-CREDENTIAL-PACKET";

    let metadata = payload_metadata(payload);

    assert!(!metadata.contains("SENTINEL-RAW-BLE-CREDENTIAL-PACKET"));
    let hex: String = payload.iter().map(|b| format!("{b:02x}")).collect();
    assert!(!metadata.contains(&hex));
    assert_eq!(metadata, format!("{} bytes", payload.len()));
    assert_eq!(
        payload_parts_metadata(&[&payload[..10], &payload[10..]]),
        metadata
    );
}

/// `test_command_metadata_never_contains_psk_arguments`.
#[test]
fn command_metadata_never_contains_psk_arguments() {
    let password = "SENTINEL-WIFI-PSK";
    let command = [
        "nmcli",
        "connection",
        "modify",
        "private-network",
        "802-11-wireless-security.psk",
        password,
    ];

    let metadata = command_metadata(&command);

    assert!(!metadata.contains(password));
    assert!(!metadata.contains("private-network"));
    assert_eq!(metadata, "nmcli (5 args)");
    assert_eq!(command_metadata(&["/usr/bin/nmcli"]), "nmcli (0 args)");
    assert_eq!(command_metadata::<&str>(&[]), "<empty command>");
}

/// `test_exception_metadata_never_contains_exception_message`.
#[test]
fn exception_metadata_never_contains_exception_message() {
    #[derive(Debug)]
    struct RuntimeError(#[allow(dead_code)] String);

    let password = "SENTINEL-EXCEPTION-PASSWORD";
    let error = RuntimeError(format!("connection failed with password {password}"));

    let metadata = exception_metadata(&error);

    assert!(!metadata.contains(password));
    assert_eq!(metadata, "RuntimeError");
}
