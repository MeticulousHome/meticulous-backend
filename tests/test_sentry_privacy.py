from sentry_privacy import sanitize_esp_data, sanitize_sentry_event


def test_sanitize_esp_data_keeps_allowlist_and_bounds_stage_name():
    sanitized = sanitize_esp_data(
        {
            "stage_name": " Infusion\x00" + ("x" * 200),
            "context": "applied_setpoint",
            "requested": "12.5",
            "password": "secret",
            "future": "value",
            "reason": "bad,value",
        }
    )

    assert sanitized["stage_name"].startswith("Infusion")
    assert len(sanitized["stage_name"]) == 128
    assert sanitized["context"] == "applied_setpoint"
    assert sanitized["requested"] == "12.5"
    assert "password" not in sanitized
    assert "future" not in sanitized
    assert "reason" not in sanitized


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
                "password": "secret",
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
    }
    assert sanitized["contexts"]["runtime"] == {"version": "3.11"}
    assert sanitized["extra"] == {"diagnostic": {"value": 2}}
    assert "vars" not in sanitized["exception"]["values"][0]["stacktrace"]["frames"][0]

    assert event["server_name"] == "private-host"
    assert "vars" in event["exception"]["values"][0]["stacktrace"]["frames"][0]
