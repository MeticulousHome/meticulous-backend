"""No test may send events to the production Sentry projects (see conftest.py)."""

import sentry_sdk

import machine


def test_the_esp32_sentry_client_has_no_dsn_and_no_transport():
    assert machine.ESPSentryClient.dsn is None
    assert machine.ESPSentryClient.transport is None


def test_a_client_built_with_a_dsn_still_sends_nothing():
    client = sentry_sdk.Client(dsn="https://key@sentry.meticulousespresso.com/3")

    assert client.dsn is None
    assert client.transport is None
