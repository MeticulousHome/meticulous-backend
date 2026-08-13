from types import SimpleNamespace

import pytest
import tornado.web
from tornado.httputil import HTTPHeaders
from tornado.testing import AsyncHTTPTestCase

from api.base_handler import LocalAccessHandler


class LocalOnlyTestHandler(LocalAccessHandler):
    def get(self):
        self.write({"status": "success"})


class TestLocalAccessHandlerHTTP(AsyncHTTPTestCase):
    def get_app(self):
        return tornado.web.Application([(r"/local", LocalOnlyTestHandler)])

    def test_prepare_allows_loopback_request(self):
        assert self.fetch("/local").code == 200

    def test_prepare_rejects_remote_forwarded_address(self):
        response = self.fetch(
            "/local", headers={"X-Real-IP": "203.0.113.42"}, raise_error=False
        )

        assert response.code == 403


def request(peer_address, forwarded_address=None):
    headers = HTTPHeaders()
    if forwarded_address is not None:
        headers["X-Real-IP"] = forwarded_address
    return SimpleNamespace(remote_ip=peer_address, headers=headers)


@pytest.mark.parametrize("peer_address", ["127.0.0.1", "127.42.0.9", "::1"])
def test_local_access_allows_genuine_loopback_peers(peer_address):
    assert LocalAccessHandler._request_is_local(request(peer_address))


def test_local_access_rejects_direct_remote_peer():
    assert not LocalAccessHandler._request_is_local(request("203.0.113.42"))


def test_local_access_rejects_proxied_remote_peer():
    assert not LocalAccessHandler._request_is_local(
        request("127.0.0.1", forwarded_address="203.0.113.42")
    )


def test_local_access_rejects_spoofed_loopback_header_from_remote_peer():
    assert not LocalAccessHandler._request_is_local(
        request("203.0.113.42", forwarded_address="127.0.0.1")
    )


def test_local_access_allows_loopback_proxy_for_loopback_caller():
    assert LocalAccessHandler._request_is_local(
        request("127.0.0.1", forwarded_address="::1")
    )


@pytest.mark.parametrize("forwarded_address", ["localhost", "unknown", ""])
def test_local_access_rejects_non_address_forwarded_values(forwarded_address):
    assert not LocalAccessHandler._request_is_local(
        request("127.0.0.1", forwarded_address=forwarded_address)
    )
