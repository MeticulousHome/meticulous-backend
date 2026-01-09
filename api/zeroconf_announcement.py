import zeroconf
import os

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

# FIXME: remove once the tornado server logic moved to its own file
ZEROCONF_PORT = int(os.getenv("ZEROCONF_PORT", "80"))


class ZeroConfAnnouncement:
    def __init__(self, config_function) -> None:
        self.zeroconf = None
        self.http_service_info = None
        self.met_service_info = None
        self.network_config = None
        self.config_function = config_function
        self.zeroconf = zeroconf.Zeroconf(
            interfaces=zeroconf.InterfaceChoice.All, ip_version=zeroconf.IPVersion.All
        )

    def _createServiceConfig(self):
        self.network_config = self.config_function()
        if not self.network_config.connected:
            logger.info("Not connected to a network, not starting zeroconf")
            return

        ips = list(map(lambda ip: str(ip.ip), self.network_config.ips))
        machine_name = self.network_config.hostname

        # Create the http service information
        self.http_service_info = zeroconf.ServiceInfo(
            "_http._tcp.local.",
            f"{machine_name}._http._tcp.local.",
            parsed_addresses=ips,
            port=ZEROCONF_PORT,
            # We can announce arbitrary information here (e.g. version numbers or features or state)
            properties={
                "server_name": machine_name,
                "ips": ips,
                "domain": self.network_config.domains,
                "machine_name": machine_name,
            },
            server=f"{machine_name}.local.",
        )

        self.met_service_info = zeroconf.ServiceInfo(
            "_meticulous._tcp.local.",
            f"{machine_name}._meticulous._tcp.local.",
            parsed_addresses=ips,
            port=ZEROCONF_PORT,
            # We can announce arbitrary information here (e.g. version numbers or features or state)
            properties={
                "server_name": machine_name,
                "ips": ips,
                "domain": self.network_config.domains,
                "machine_name": machine_name,
            },
            server=f"{machine_name}.local.",
        )

    def start(self):
        logger.info("Registering Service with zeroconf")
        # Register the service
        try:
            self.network_config = self.config_function()
            self._createServiceConfig()
            if self.http_service_info is not None:
                self.zeroconf.register_service(
                    self.http_service_info, allow_name_change=True
                )
                logger.info(f"zeroconf service http announced on port {ZEROCONF_PORT}")
            else:
                logger.warning("Could not fetch machine informations for http zeroconf")
            if self.met_service_info is not None:
                self.zeroconf.register_service(
                    self.met_service_info, allow_name_change=True
                )
                logger.info(f"zeroconf service meticulous announced on port {ZEROCONF_PORT}")
            else:
                logger.warning(
                    "Could not fetch machine informations for meticulous zeroconf"
                )

            return
        except zeroconf.NonUniqueNameException:
            logger.warning(
                f"zeroconf failed to start on port {ZEROCONF_PORT} error='NonUniqueNameException'"
            )

    def stop(self):
        if self.met_service_info is not None:
            # Unregister the service
            self.zeroconf.unregister_service(self.met_service_info)
            logger.info("zeroconf meticulous stopped")
            self.met_service_info = None

        if self.http_service_info is not None:
            # Unregister the service
            self.zeroconf.unregister_service(self.http_service_info)
            logger.info("zeroconf http stopped")
            self.http_service_info = None

    def restart(self):
        self.stop()
        self.start()
