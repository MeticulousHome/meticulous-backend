from sensitive_logging import (
    command_metadata,
    credential_metadata,
    exception_metadata,
    payload_metadata,
)


def test_credential_metadata_never_contains_unicode_credentials():
    ssid = "Private-网络"
    password = "SENTINEL-密码-pässword"

    metadata = credential_metadata(ssid, password)

    assert ssid not in metadata
    assert password not in metadata
    assert metadata == "ssid_bytes=14, password_bytes=25"


def test_payload_metadata_never_contains_raw_packet_bytes():
    payload = bytearray(b"SENTINEL-RAW-BLE-CREDENTIAL-PACKET")

    metadata = payload_metadata(payload)

    assert payload.decode() not in metadata
    assert payload.hex() not in metadata
    assert metadata == f"{len(payload)} bytes"


def test_command_metadata_never_contains_psk_arguments():
    password = "SENTINEL-WIFI-PSK"
    command = [
        "nmcli",
        "connection",
        "modify",
        "private-network",
        "802-11-wireless-security.psk",
        password,
    ]

    metadata = command_metadata(command)

    assert password not in metadata
    assert "private-network" not in metadata
    assert metadata == "nmcli (5 args)"


def test_exception_metadata_never_contains_exception_message():
    password = "SENTINEL-EXCEPTION-PASSWORD"
    error = RuntimeError(f"connection failed with password {password}")

    metadata = exception_metadata(error)

    assert password not in metadata
    assert metadata == "RuntimeError"
