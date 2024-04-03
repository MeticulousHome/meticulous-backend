import tornado.ioloop
import tornado.web
from config import *
from netaddr import IPAddress, IPNetwork


class BaseHandler(tornado.web.RequestHandler):
    def set_default_headers(self):
        # FIXME: I know this is not great, you know this isn't great. What shall we do about this?
        self.set_header("Access-Control-Allow-Origin", "*")
        self.set_header('Content-type', 'application/json')
        self.set_header('Access-Control-Allow-Methods',
                        'GET,POST,OPTIONS,DELETE')
        self.set_header('Access-Control-Allow-Headers', 'content-type')

    def options(self):
        pass

    def prepare(self):

        return

        # Skip the check if the request is from localhost
        if self.request.remote_ip == "127.0.0.1" and self.request.remote_ip == "::1":
            return

        if MeticulousConfig[CONFIG_WIFI][WIFI_MODE] == WIFI_MODE_AP:
            return

        allowed_networks = [IPNetwork(
            x) for x in MeticulousConfig[CONFIG_SYSTEM][SYSTEM_ALLOWED_NETWORKS_NAME]]

        # TODO test me well!
        if len([network for network in allowed_networks if self.request.remote_ip in network]) > 0:
            return

        # Validate the X-Authorized header
        x_authorized = self.request.headers.get("X-Authorized")
        if not x_authorized or x_authorized != MeticulousConfig[CONFIG_SYSTEM][SYSTEM_AUTH_KEY_NAME]:
            self.set_status(401)
            self.finish("Unauthorized: Missing X-Authorized header")
            return
