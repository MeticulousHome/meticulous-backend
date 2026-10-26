from ipaddress import IPv6Address, ip_address

import tornado.web
from netaddr import IPNetwork

from config import (
    CONFIG_SYSTEM,
    CONFIG_WIFI,
    HTTP_ALLOWED_NETWORKS,
    HTTP_AUTH_KEY,
    WIFI_MODE,
    WIFI_MODE_AP,
    MeticulousConfig,
)
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


def redact_ip(ip: str) -> str:
    """Keeps only the first two segments of an IPv4 or IPv6 address.

    i.e 192.168.1.42 -> 192.168.x.xx, 2001:db8:1:2::7 -> 2001:db8:x:x:x:x
    """
    separator = ":" if ":" in ip else "."

    return separator.join(
        segment if index < 2 else "x" * max(len(segment), 1)
        for index, segment in enumerate(ip.split(separator))
    )


class BaseHandler(tornado.web.RequestHandler):
    def set_default_headers(self):
        # FIXME: I know this is not great, you know this isn't great. What shall we do about this?
        self.set_header("Access-Control-Allow-Origin", "*")
        self.set_header("Access-Control-Allow-Headers", "*")
        self.set_header("Access-Control-Expose-Headers", "*")

        self.set_header("Content-type", "application/json")
        self.set_header("Access-Control-Allow-Methods", "GET,POST,PUT,OPTIONS,DELETE")
        self.set_header(
            "Access-Control-Allow-Headers", "content-type, authorization, x-authorized"
        )
        # We hate caching!
        self.set_header("Cache-Control", "no-cache")
        self.set_header("Pragma", "no-cache")
        self.set_header("Expires", "0")

    def report_error(self, error_code, error: str, error_details=None):
        self.set_status(error_code)
        self.write({"error": error, "details": error_details})

    def options(self, *args, **kwargs):
        # No body for OPTIONS requests
        self.set_status(204)
        self.finish()

    def prepare(self):

        return

        # Skip the check if the request is from localhost
        if self.request.remote_ip == "127.0.0.1" and self.request.remote_ip == "::1":
            return

        if MeticulousConfig[CONFIG_WIFI][WIFI_MODE] == WIFI_MODE_AP:
            return

        allowed_networks = [
            IPNetwork(x) for x in MeticulousConfig[CONFIG_SYSTEM][HTTP_ALLOWED_NETWORKS]
        ]

        # TODO test me well!
        if (
            len([network for network in allowed_networks if self.request.remote_ip in network])
            > 0
        ):
            return

        # Validate the X-Authorized header
        x_authorized = self.request.headers.get("X-Authorized")
        if not x_authorized or x_authorized != MeticulousConfig[CONFIG_SYSTEM][HTTP_AUTH_KEY]:
            self.set_status(401)
            self.finish("Unauthorized: Missing X-Authorized header")
            return


def is_loopback_address(address: str) -> bool:
    """Whether address is a loopback IP: 127.0.0.0/8, ::1 or ::ffff:127.x.x.x."""
    try:
        ip = ip_address(address)
    except ValueError:
        return False
    if isinstance(ip, IPv6Address) and ip.ipv4_mapped:
        return ip.ipv4_mapped.is_loopback
    return ip.is_loopback


class LocalAccessHandler(BaseHandler):
    """Base handler that restricts access to local requests only.

    The backend only listens on 127.0.0.1 (backend.py), so a request either comes
    straight from a process on the machine, without X-Real-IP, or through the
    machine's nginx, which sets X-Real-IP to the client's address. Only that address
    decides: the Host header is whatever the client sends.
    """

    def prepare(self):
        super().prepare()
        remote_ip = self.request.headers.get("X-Real-IP")
        if remote_ip is not None and not is_loopback_address(remote_ip):
            logger.warning(
                f"Unauthorized access to {self.request.uri} "
                f"from remote IP: {redact_ip(remote_ip)}"
            )
            self.set_status(403)
            self.write(
                {
                    "status": "error",
                    "error": "This endpoint can only be accessed locally",
                }
            )
            self.finish()
            return
