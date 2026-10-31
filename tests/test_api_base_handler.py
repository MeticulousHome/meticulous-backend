"""Which requests LocalAccessHandler lets through.

Protects every local-only route at once (root password, factory reset, smoke
validation, the upload index, writing Pour Over history): they share this check.
The backend listens on 127.0.0.1 only (backend.py), so a request without
X-Real-IP comes from a process on the machine (the Dial calls
http://localhost:8080), and nginx sets X-Real-IP to $remote_addr for everything
else. The expected 403 body comes from LocalAccessHandler in api/base_handler.py.

The Host header is chosen by the client and nginx forwards it as is, so it must
not decide: a LAN client addressing the machine as localhost is still remote.
"""

import json

import pytest
from tornado.testing import AsyncHTTPTestCase
from tornado.web import Application

from api.base_handler import LocalAccessHandler, is_loopback_address

FORBIDDEN = {"status": "error", "error": "This endpoint can only be accessed locally"}


class LocalOnly(LocalAccessHandler):
    def get(self):
        self.write({"reached": True})


class TestLocalAccessHandler(AsyncHTTPTestCase):
    def get_app(self):
        return Application([(r"/local", LocalOnly)])

    def assert_forbidden(self, headers):
        response = self.fetch("/local", headers=headers)
        assert response.code == 403, headers
        assert json.loads(response.body) == FORBIDDEN

    def assert_reached(self, headers):
        response = self.fetch("/local", headers=headers)
        assert response.code == 200, headers
        assert json.loads(response.body) == {"reached": True}

    def test_lan_client_is_forbidden(self):
        self.assert_forbidden({"X-Real-IP": "192.168.1.42", "Host": "meticulous.local"})

    def test_lan_client_addressing_the_machine_as_localhost_is_forbidden(self):
        for host in ("localhost", "127.0.0.1", "localhost:80", "[::1]"):
            self.assert_forbidden({"X-Real-IP": "192.168.1.42", "Host": host})

    def test_ipv6_lan_client_addressing_the_machine_as_localhost_is_forbidden(self):
        self.assert_forbidden({"X-Real-IP": "fe80::1c2b:3aff:fe4d:5e6f", "Host": "localhost"})

    def test_request_without_the_proxy_header_is_reached(self):
        # Straight to 127.0.0.1:8080, the way the Dial calls the backend.
        for host in ("localhost:8080", "127.0.0.1:8080", "meticulous.local"):
            self.assert_reached({"Host": host})

    def test_loopback_client_through_the_proxy_is_reached(self):
        for address in ("127.0.0.1", "::1"):
            self.assert_reached({"X-Real-IP": address, "Host": "meticulous.local"})

    def test_proxy_header_that_is_not_an_address_is_forbidden(self):
        # nginx always sends an address; anything else is refused rather than trusted.
        for value in ("localhost", "unknown", ""):
            self.assert_forbidden({"X-Real-IP": value, "Host": "localhost"})


@pytest.mark.parametrize(
    "address", ["127.0.0.1", "127.0.1.1", "::1", "::ffff:127.0.0.1"], ids=str
)
def test_loopback_addresses(address):
    assert is_loopback_address(address)


@pytest.mark.parametrize(
    "address",
    ["192.168.1.42", "10.0.0.7", "fe80::1", "::ffff:192.168.1.42", "localhost", ""],
    ids=repr,
)
def test_non_loopback_addresses(address):
    assert not is_loopback_address(address)
