from sentry_privacy import sanitize_sentry_event


def test_sanitize_event_preserves_complete_esp_diagnostics():
    esp_data = {
        "stage_name": "Infusion " + ("x" * 200),
        "future_sensor": "raw=value/with,punctuation",
        "diagnostic_code": "HW-42",
    }

    sanitized = sanitize_sentry_event({"contexts": {"esp-data": esp_data}})

    assert sanitized["contexts"]["esp-data"] == esp_data
    assert sanitized["contexts"]["esp-data"] is not esp_data


def test_sanitize_event_removes_automatic_personal_context():
    event = {
        "server_name": "private-host",
        "user": {"id": "person"},
        "request": {
            "headers": {"Authorization": "Bearer secret"},
            "cookies": {"session": "secret"},
        },
        "breadcrumbs": {"values": [{"message": "Private SSID"}]},
        "tags": {
            "machine": "private-machine",
            "hostname": "private-host",
            "serial": "M123",
        },
        "contexts": {
            "config": {
                "version": 1,
                "system": {
                    "serial": "M123",
                    "sounds_theme": "private-theme",
                    "root_password": "secret",
                },
                "wifi": {"mode": "CLIENT", "APName": "Private SSID"},
            },
            "esp-data": {
                "stage_name": "Infusion",
                "requested": "10.0",
                "future_sensor": "raw=value/with,punctuation",
            },
            "runtime": {
                "hostname": "private-host",
                "ip": "192.0.2.1",
                "version": "3.11",
            },
        },
        "extra": {
            "authorization": "secret",
            "password_hash": "secret",
            "diagnostic": {"value": 2},
        },
        "exception": {
            "values": [
                {
                    "stacktrace": {
                        "frames": [
                            {
                                "function": "connect",
                                "vars": {"password": "secret"},
                            }
                        ]
                    }
                }
            ]
        },
    }

    sanitized = sanitize_sentry_event(event)

    assert "server_name" not in sanitized
    assert "user" not in sanitized
    assert "request" not in sanitized
    assert "breadcrumbs" not in sanitized
    assert sanitized["tags"] == {"serial": "M123"}
    assert sanitized["contexts"]["config"] == {
        "version": 1,
        "system": {"serial": "M123", "sounds_theme": "custom"},
        "wifi": {"mode": "CLIENT"},
    }
    assert sanitized["contexts"]["esp-data"] == {
        "stage_name": "Infusion",
        "requested": "10.0",
        "future_sensor": "raw=value/with,punctuation",
    }
    assert sanitized["contexts"]["runtime"] == {"version": "3.11"}
    assert sanitized["extra"] == {"diagnostic": {"value": 2}}
    assert "vars" not in sanitized["exception"]["values"][0]["stacktrace"]["frames"][0]

    assert event["server_name"] == "private-host"
    assert "vars" in event["exception"]["values"][0]["stacktrace"]["frames"][0]
