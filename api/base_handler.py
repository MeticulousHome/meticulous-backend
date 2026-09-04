import tornado.web

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
        # Reflect the requesting Origin instead of a literal "*". CORS is not the
        # security boundary here -- the bearer token is (see api/auth.py). We stay
        # permissive so first-party and community web tools keep working, but
        # reflecting the origin (rather than "*") lets us expose specific headers
        # and never sets Access-Control-Allow-Credentials (we use a header token,
        # not cookies, which also keeps us immune to CSRF).
        origin = self.request.headers.get("Origin")
        if origin:
            self.set_header("Access-Control-Allow-Origin", origin)
            self.set_header("Vary", "Origin")
        else:
            self.set_header("Access-Control-Allow-Origin", "*")
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

    # Authorization is enforced uniformly by AuthMixin (api/auth.py), which is
    # prepended to every route in API.get_routes(). BaseHandler no longer carries
    # its own auth check.


class LocalAccessHandler(BaseHandler):
    """Base handler that restricts access to local requests only."""

    def prepare(self):
        super().prepare()
        # Locality is decided ONLY from the proxy-set peer address, never from
        # the caller-controlled Host header: nginx overwrites X-Real-IP, but a
        # LAN client can send `Host: localhost` freely. Trusting Host here let a
        # remote (or stolen-token) client read the root password and trigger a
        # factory reset.
        from .auth import request_is_local

        if not request_is_local(self):
            remote_ip = self.request.headers.get("X-Real-IP")
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
