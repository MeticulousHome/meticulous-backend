"""Regression coverage for stable customer-visible machine identity."""

import importlib.util
import sys
from pathlib import Path
from types import ModuleType
from unittest.mock import MagicMock, patch


class _Config(dict):
    def __init__(self, value):
        super().__init__(value)
        self.save = MagicMock()


def _module(name: str, **attributes):
    module = ModuleType(name)
    for attribute, value in attributes.items():
        setattr(module, attribute, value)
    return module


def _load_hostname_module(monkeypatch, identifier=None, hostname_override=None):
    config = _Config(
        {
            "system": {
                "machine_name": [] if identifier is None else identifier,
                "serial": "003312",
            },
            "user": {"hostname_override": hostname_override},
        }
    )
    config_module = _module(
        "config",
        CONFIG_SYSTEM="system",
        CONFIG_USER="user",
        DEVICE_IDENTIFIER="machine_name",
        HOSTNAME_OVERRIDE="hostname_override",
        MACHINE_SERIAL_NUMBER="serial",
        MeticulousConfig=config,
    )
    logger = MagicMock()
    log_module = _module("log", MeticulousLogger=MagicMock())
    log_module.MeticulousLogger.getLogger.return_value = logger
    monkeypatch.setitem(sys.modules, "config", config_module)
    monkeypatch.setitem(sys.modules, "log", log_module)

    module_name = "hostname_preservation_under_test"
    module_path = Path(__file__).resolve().parents[1] / "hostname.py"
    spec = importlib.util.spec_from_file_location(module_name, module_path)
    module = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, module_name, module)
    spec.loader.exec_module(module)
    return module, config, logger


def test_identifier_is_recovered_from_established_hostname(monkeypatch):
    module, config, logger = _load_hostname_module(monkeypatch)

    with (
        patch.object(module.socket, "gethostname", return_value="meticulousSpicyCrema-003312"),
        patch.object(
            module.HostnameManager,
            "_generateRandomIdentifierComponents",
            side_effect=AssertionError("must not generate a new identity"),
        ),
    ):
        module.HostnameManager.init()

    assert config["system"]["machine_name"] == ["spicy", "Crema"]
    assert module.HostnameManager.generateDeviceName() == "MeticulousSpicyCrema"
    assert module.HostnameManager.generateHostname() == "meticulousSpicyCrema-003312"
    config.save.assert_called_once_with()
    logger.warning.assert_called_once_with(
        "Recovered device identifier from established hostname"
    )


def test_matching_existing_identifier_is_not_rewritten(monkeypatch):
    module, config, _ = _load_hostname_module(monkeypatch, ["spicy", "Crema"])

    with patch.object(module.socket, "gethostname", return_value="meticulousSpicyCrema-003312"):
        module.HostnameManager.init()

    assert config["system"]["machine_name"] == ["spicy", "Crema"]
    config.save.assert_not_called()


def test_conflicting_identifier_is_repaired_from_established_hostname(monkeypatch):
    module, config, logger = _load_hostname_module(monkeypatch, ["renowned", "Body"])

    with patch.object(module.socket, "gethostname", return_value="meticulousSpicyCrema-003312"):
        module.HostnameManager.init()

    assert config["system"]["machine_name"] == ["spicy", "Crema"]
    config.save.assert_called_once_with()
    logger.warning.assert_called_once_with(
        "Recovered device identifier from established hostname"
    )


def test_explicit_override_keeps_configured_identifier(monkeypatch):
    module, config, _ = _load_hostname_module(
        monkeypatch,
        ["renowned", "Body"],
        hostname_override="custom-machine",
    )

    with patch.object(module.socket, "gethostname", return_value="meticulousSpicyCrema-003312"):
        module.HostnameManager.init()

    assert config["system"]["machine_name"] == ["renowned", "Body"]
    config.save.assert_not_called()


def test_factory_hostname_without_identity_gets_new_identifier(monkeypatch):
    module, config, _ = _load_hostname_module(monkeypatch)

    with (
        patch.object(module.socket, "gethostname", return_value="imx8mn-var-som"),
        patch.object(
            module.HostnameManager,
            "_generateRandomIdentifierComponents",
            return_value=("balanced", "Bloom"),
        ),
    ):
        module.HostnameManager.init()

    assert config["system"]["machine_name"] == ["balanced", "Bloom"]
    config.save.assert_called_once_with()


def test_non_generated_hostname_is_not_misparsed(monkeypatch):
    module, _, _ = _load_hostname_module(monkeypatch)

    assert module.HostnameManager.identifierFromHostname("custom-machine") is None
    assert module.HostnameManager.identifierFromHostname("meticulousSpicyCrema-999999") is None


def test_every_generated_identifier_round_trips_through_hostname(monkeypatch):
    module, config, _ = _load_hostname_module(monkeypatch)

    for adjective in module.HostnameManager.ADJECTIVES:
        for noun in module.HostnameManager.NOUNS:
            config["system"]["machine_name"] = [adjective, noun]
            hostname = module.HostnameManager.generateHostname()
            assert module.HostnameManager.identifierFromHostname(hostname) == (
                adjective,
                noun,
            )
