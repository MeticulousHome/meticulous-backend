"""Which API routes refuse LAN clients.

LocalAccessHandler answers 403 to a request that neither comes from nor is
addressed to the machine itself. LOCAL_ONLY lists every route and method that
does so; everything else serves LAN clients (Community, the mobile app). A
method refused but not listed fails, and so does a listed one that is served, so
the list only changes deliberately and the change is visible in review.

Each handler's methods are replaced by a stub, so only the access check in
prepare() runs: no request reaches the machine, the database or the disk.
"""

import json

from tornado.testing import AsyncHTTPTestCase
from tornado.web import Application, RequestHandler

from api.api import API

LAN_CLIENT = {"Host": "meticulous.local", "X-Real-IP": "192.168.1.42"}
VERBS = ("get", "post", "put", "patch", "delete", "options")

LOCAL_ONLY = {
    # The Dial writes Pour Over history; LAN clients only read and sync it.
    "POST /api/v1/history/pour-over",
    "GET /api/v1/history/upload-index",
    "GET /api/v1/machine/factory_reset",
    "GET /api/v1/machine/root-password",
    "GET /api/v1/smoke-validation",
}


def served_methods(handler):
    return [
        verb for verb in VERBS if getattr(handler, verb) is not getattr(RequestHandler, verb)
    ]


def reached(self, *args, **kwargs):
    self.write({"reached": True})


def probe(handler):
    """The handler with its access check and every method replaced by a stub."""
    stubs = {verb: reached for verb in VERBS}
    # StaticFileHandler's ETag reads the file get() would have resolved.
    stubs["compute_etag"] = RequestHandler.compute_etag
    return type(f"Probe{handler.__name__}", (handler,), stubs)


class TestLocalAccessPolicy(AsyncHTTPTestCase):
    def get_app(self):
        # Probes are mounted at literal paths so regex routes need no sample URL.
        self.routes = API.get_routes()
        return Application(
            [
                (f"/probe/{index}", probe(handler), kwargs)
                for index, (_, handler, kwargs) in enumerate(self.routes)
            ]
        )

    def lan_refusals(self):
        refused = set()
        for index, (path, handler, _) in enumerate(self.routes):
            for verb in served_methods(handler):
                method = verb.upper()
                response = self.fetch(
                    f"/probe/{index}",
                    method=method,
                    headers=LAN_CLIENT,
                    body=b"" if method in ("POST", "PUT", "PATCH") else None,
                )
                if response.code == 403:
                    refused.add(f"{method} {path}")
                else:
                    assert json.loads(response.body) == {"reached": True}, (method, path)
        return refused

    def test_only_the_listed_methods_refuse_lan_clients(self):
        refused = {route for route in self.lan_refusals() if not route.startswith("OPTIONS")}
        assert sorted(refused - LOCAL_ONLY) == [], "refused to LAN clients, not in LOCAL_ONLY"
        assert sorted(LOCAL_ONLY - refused) == [], "in LOCAL_ONLY, served to LAN clients"

    def test_a_route_lan_clients_may_call_answers_their_preflight(self):
        refused = self.lan_refusals()
        missing = sorted(
            f"OPTIONS {path}"
            for path, handler, _ in self.routes
            if "options" in served_methods(handler)
            and f"OPTIONS {path}" in refused
            and any(
                f"{verb.upper()} {path}" not in refused
                for verb in served_methods(handler)
                if verb != "options"
            )
        )
        assert missing == []
