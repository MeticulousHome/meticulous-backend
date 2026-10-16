"""Regression tests for the Wi-Fi health cache.

Covers the three properties the shallow/deep split depends on: a shallow entry
never answers a deep caller, the fast path never blocks on the probe lock, and
the cache entry is written as one tuple so it cannot tear across two builds.
"""

import threading
from types import SimpleNamespace
from unittest.mock import patch

from tests.test_wifi_repair import _load_wifi_module, _wifi_config


def _connected(module):
    config = _wifi_config(module, connected=True, connection_name="Home")
    config.ips = [SimpleNamespace(ip=SimpleNamespace(version=4))]
    return config


def _probes(manager, gateway=True, dns=True, internet=True):
    return (
        patch.object(manager, "gatewayReachable", return_value=gateway),
        patch.object(manager, "dnsResolves", return_value=dns),
        patch.object(manager, "internetReachable", return_value=internet),
    )


def test_shallow_entry_is_never_served_to_a_deep_caller(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    manager.invalidateHealthCache()
    current = _connected(module)

    with patch.object(manager, "refreshHealthInBackground"):
        shallow = manager.getHealthStatus(current, deep=False)

    assert shallow.verified is False
    assert shallow.internet_reachable is False  # unknown, not "unreachable"

    gateway, dns, internet = _probes(manager)
    with gateway as g, dns as d, internet as i:
        deep = manager.getHealthStatus(current, deep=True)

    # The deep caller must have run the probes instead of reusing the shallow entry.
    g.assert_called_once()
    d.assert_called_once()
    i.assert_called_once()
    assert deep.verified is True
    assert deep.internet_reachable is True


def test_shallow_caller_reuses_a_fresh_deep_entry(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    manager.invalidateHealthCache()
    current = _connected(module)

    gateway, dns, internet = _probes(manager)
    with gateway, dns, internet:
        manager.getHealthStatus(current, deep=True)

    with patch.object(manager, "refreshHealthInBackground") as refresh:
        shallow = manager.getHealthStatus(current, deep=False)

    # Served straight from the deep entry: no rebuild, no background refresh.
    assert shallow.verified is True
    assert shallow.internet_reachable is True
    refresh.assert_not_called()


def test_shallow_path_does_not_block_on_the_probe_lock(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    manager.invalidateHealthCache()
    current = _connected(module)
    result = {}

    # Stand in for a deep probe in flight, holding the lock for a long time.
    manager._health_check_lock.acquire()
    try:

        def call():
            with patch.object(manager, "refreshHealthInBackground"):
                result["health"] = manager.getHealthStatus(current, deep=False)

        worker = threading.Thread(target=call)
        worker.start()
        worker.join(timeout=2)
        assert not worker.is_alive(), "shallow path blocked on the deep probe lock"
    finally:
        manager._health_check_lock.release()

    assert result["health"].verified is False


def test_concurrent_shallow_and_deep_builds_never_tear_the_cache(monkeypatch):
    """The cached health, timestamp and signature must come from one build.

    They used to be three separate assignments, so a shallow build racing a deep
    one could leave a health object paired with the other build's signature.
    """
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    manager.invalidateHealthCache()

    home, guest = _connected(module), _connected(module)
    guest.connection_name = "Guest"
    home_signature = manager.getHealthCacheSignature(home)
    guest_signature = manager.getHealthCacheSignature(guest)
    assert home_signature != guest_signature

    stop = threading.Event()

    def shallow_loop():
        while not stop.is_set():
            manager.buildHealthStatus(home, deep=False)

    gateway, dns, internet = _probes(manager)
    with gateway, dns, internet:
        worker = threading.Thread(target=shallow_loop, daemon=True)
        worker.start()
        try:
            for _ in range(20000):
                manager.buildHealthStatus(guest, deep=True)
                health, cached_at, signature = manager._health_cache
                expected = guest_signature if health.verified else home_signature
                assert signature == expected, "cache entry tore across two builds"
                assert cached_at > 0
        finally:
            stop.set()
            worker.join(timeout=2)


def test_cache_entry_is_written_as_one_tuple(monkeypatch):
    module = _load_wifi_module(monkeypatch)
    manager = module.WifiManager
    manager.invalidateHealthCache()
    current = _connected(module)

    gateway, dns, internet = _probes(manager)
    with gateway, dns, internet:
        health = manager.getHealthStatus(current, deep=True)

    entry = manager._health_cache
    assert isinstance(entry, tuple) and len(entry) == 3
    cached_health, cached_at, signature = entry
    assert cached_health is health
    assert signature == manager.getHealthCacheSignature(current)
    assert cached_at > 0
