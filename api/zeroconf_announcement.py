from config import *
from hostname import HostnameManager
import zeroconf

class ZeroConfAnnouncement:
    def __init__(self, config_function) -> None:
        self.service_type = "_http._tcp.local."
        self.zeroconf = zeroconf.Zeroconf()
        self.port = 8080
        self.service_name = "_meticulous"
        self.service_handle = None
        self.service_info = None
        self.network_config = None
        self.config_function = config_function

    def _createServiceConfig(self):
        self.network_config = self.config_function()
        if not self.network_config.connected:
            logger.info("Not connected to a network, not starting zeroconf")
            return

        ips = list(map(lambda ip: str(ip.ip), self.network_config.ips))

        # Create the service information
        self.service_info = zeroconf.ServiceInfo(
            self.service_type,
            f"{self.service_name}.{self.service_type}",
            addresses=ips,
            port=self.port,
            # We can announce arbitrary information here (e.g. version numbers or features or state)
            properties={
                'server_name': self.network_config.hostname,
                'machine_name': HostnameManager.generateHostname(self.network_config.mac)
            },
            server=f"{self.network_config.hostname}.local"
        )

    def start(self):
        if self.service_info is not None:
            return

        logger.info("Registering Service with zeroconf")
        # Register the service
        try:
            self._createServiceConfig()
            if self.service_info is not None:
                self.zeroconf.register_service(self.service_info, allow_name_change=True)
                logger.info(f"Service {self.service_name} started on port {self.port}")
            else: 
                logger.warning("Could not fetch machine informations for zeroconf")
            return
        except zeroconf.NonUniqueNameException as e:
            logger.warning(
                f"Service {self.service_name} failed to start on port {self.port} error='NonUniqueNameException'")

    def stop(self):
        if self.service_info is None:
            return
        # Unregister the service
        self.zeroconf.unregister_service(self.service_info)
        print(f"Service {self.service_name} stopped")
        self.service_info = None

    def restart(self):
        self.stop()
        self.start()
