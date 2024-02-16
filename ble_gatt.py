from typing import Dict, Optional
import sys
import asyncio
import psutil
import time
from threading import Thread

from improv import *
from bless import (  # type: ignore
    BlessServer,
    BlessGATTCharacteristic,
    GATTCharacteristicProperties,
    GATTAttributePermissions,
)

from wifi import WifiManager
from config import *
from hostname import HostnameManager

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

# NOTE: Some systems require different synchronization methods.
if sys.platform in ["darwin", "win32"]:
    raise ValueError("Cannot run on non-linux platforms")

# FIXME Remove once the tornado server logic is in its own class
PORT = int(os.getenv("PORT", '8080'))

class GATTServer:
    """
    A class representing a BLE (Bluetooth Low Energy) GATT (Generic Attribute Profile) Server
    for Wi-Fi provisioning purposes. This server allows a BLE client to interact with the server
    to perform tasks like scanning for Wi-Fi networks and connecting to them.

    Attributes:
        GATT_NAME (str): The name of the GATT server. It defaults to 'MeticulousEspresso' or
                         can be set via the GATT_NAME environment variable.

    Methods:
        getServer() -> GATTServer:
            Static method to get the singleton instance of the GATTServer.

        allow_wifi_provisioning():
            Authorizes the server for Wi-Fi provisioning if it is awaiting authorization.

        is_running() -> bool:
            Checks if the server's loop thread is alive and running.

        start():
            Starts the BLE GATT Server. Initializes and runs the server loop in a separate thread.

        stop():
            Stops the BLE GATT Server. Signals the server loop to exit.

        wifi_connect(ssid: str, passwd: str) -> Optional[list[str]]:
            Static method to initiate connection to a Wi-Fi network with the given SSID and password.
            Returns a list of URLs/IPs for the local server if successful.

        get_wifi_networks() -> Optional[list[str]]:
            Static method to scan for available Wi-Fi networks and return their SSIDs.

        read_request(characteristic: BlessGATTCharacteristic, **kwargs) -> bytearray:
            Handles read requests from a BLE client for a given characteristic.

        write_request(characteristic: BlessGATTCharacteristic, value: bytearray, **kwargs):
            Handles write requests from a BLE client for a given characteristic.
    """

    MIN_BOOT_TIME = 60

    _singletonServer = None

    def __init__(self):
        self.trigger = asyncio.Event()
        self.loop = asyncio.new_event_loop()

        def _exception_handler(loop, context):
            logger.exception("GATT Server crashed!", exc_info=context, stack_info=True)
            GATTServer.getServer().stop()

        self.loop.set_exception_handler(_exception_handler)
        self.improv_server = ImprovProtocol(
            requires_authorization=False,
            wifi_connect_callback=GATTServer.wifi_connect,
            wifi_networks_callback=GATTServer.get_wifi_networks,
            max_response_bytes=250,
        )
        config = WifiManager.getCurrentConfig()
        # Only update the hostname if it is a new system or if the hostname has been
        # set before. Do so in case the lookup table ever changed or the hostname is only
        # saved transient
        if config.mac != "":
            (host_adjective, host_noun) = HostnameManager.generateHostnameComponents(config.mac)
            self.gatt_name = "Meticulous" + host_adjective.title() + host_noun.title()
        else:
            self.gatt_name = "MeticulousEspresso"

        self.bless_gatt_server = None
        self.loopThread = None
        logger.info(f"BLE init called. Name={self.gatt_name}")


    def getServer():
        if GATTServer._singletonServer is None:
            GATTServer._singletonServer = GATTServer()
        return GATTServer._singletonServer

    def allow_wifi_provisioning(self):
        if self.improv_server.state == ImprovState.AWAITING_AUTHORIZATION:
            self.improv_server.state = ImprovState.AUTHORIZED

    def is_provisioning(self):
        return self.improv_server.state.value >= ImprovState.AUTHORIZED.value

    def is_running(self):
        if self.loopThread is None:
            return False

        return self.loopThread.is_alive()

    def start(self):
        logger.info(f"Starting BLE GATT Server")

        if not self.is_running():
            self.trigger.clear()

            def run_loop(loop):
                asyncio.set_event_loop(loop)
                loop.run_until_complete(self._ble_gatt_server_loop())

            self.loopThread = Thread(target=run_loop, args=(self.loop,))
            self.loopThread.start()

        else:
            logger.info("BLE GATT Server is already running")

    def stop(self):
        logger.info("Stopping BLE GATT Server")
        self.loop.call_soon_threadsafe(self.trigger.set)

    async def _ble_gatt_server_loop(self):
        # FIXME remove once migrated away from the variscite-wifi.service towards
        # proper sdio power sequencing
        # After boot we need to was 10 or so seconds for the variscite wifi service
        # to enable bluetooth again
        uptime_missing = round(
            GATTServer.MIN_BOOT_TIME - (time.time() - psutil.boot_time())
        )
        if uptime_missing > 0:
            logger.info(
                f"GattServer started to fast after system boot. Waiting {uptime_missing} seconds"
            )
            time.sleep(uptime_missing)

        if self.bless_gatt_server is None:
            self.bless_gatt_server = BlessServer(name=self.gatt_name, loop=self.loop)
            self.bless_gatt_server.read_request_func = GATTServer.read_request
            self.bless_gatt_server.write_request_func = GATTServer.write_request

        # Power on the hci device if it is powered off
        await self.bless_gatt_server.setup_task

        interface = self.bless_gatt_server.adapter.get_interface("org.bluez.Adapter1")
        powered = await interface.get_powered()
        if not powered:
            logger.info("bluetooth device is not powered, powering now!")
            await interface.set_powered(True)

        # Init and start GATT
        await self.bless_gatt_server.add_gatt(GATTServer._build_gatt())
        success = await self.bless_gatt_server.start()

        if not success:
            raise RuntimeError("GATT server couldn't be started")

        try:
            logger.info("GATT Server started")
            self.trigger.clear()
            await self.trigger.wait()
            logger.debug("GATT loop exiting")
            await self.bless_gatt_server.stop()

        except KeyboardInterrupt:
            logger.info("KeyboardInterrupt: Shutting Down")
        except Exception as e:
            logger.exception("GATT Server Crashed", exc_info=e, stack_info=True)
            raise
        logger.info("GATT Server stopped")

    def _build_gatt():
        gatt: Dict = {
            ImprovUUID.SERVICE_UUID.value: {
                ImprovUUID.STATUS_UUID.value: {
                    "Properties": (
                        GATTCharacteristicProperties.read
                        | GATTCharacteristicProperties.notify
                    ),
                    "Permissions": (
                        GATTAttributePermissions.readable
                        | GATTAttributePermissions.writeable
                    ),
                },
                ImprovUUID.ERROR_UUID.value: {
                    "Properties": (
                        GATTCharacteristicProperties.read
                        | GATTCharacteristicProperties.notify
                    ),
                    "Permissions": (
                        GATTAttributePermissions.readable
                        | GATTAttributePermissions.writeable
                    ),
                },
                ImprovUUID.RPC_COMMAND_UUID.value: {
                    "Properties": (
                        GATTCharacteristicProperties.read
                        | GATTCharacteristicProperties.write
                        | GATTCharacteristicProperties.write_without_response
                    ),
                    "Permissions": (
                        GATTAttributePermissions.readable
                        | GATTAttributePermissions.writeable
                    ),
                },
                ImprovUUID.RPC_RESULT_UUID.value: {
                    "Properties": (
                        GATTCharacteristicProperties.read
                        | GATTCharacteristicProperties.notify
                    ),
                    "Permissions": (GATTAttributePermissions.readable),
                },
                ImprovUUID.CAPABILITIES_UUID.value: {
                    "Properties": (GATTCharacteristicProperties.read),
                    "Permissions": (GATTAttributePermissions.readable),
                },
            }
        }
        return gatt

    def wifi_connect(ssid: str, passwd: str) -> Optional[list[str]]:
        ssid = ssid.decode("utf-8")
        passwd = passwd.decode("utf-8")
        logger.info(f"Connecting to '{ssid}' with password: '{passwd}'")
        if WifiManager.connectToWifi(ssid, passwd):
            networkConfig = WifiManager.getCurrentConfig()
            localServer = []
            for localIP in networkConfig.ips:
                if localIP.ip.version == 6:
                    localServer.append(f"http://[{str(localIP.ip)}]:{PORT}")
                else:
                    localServer.append(f"http://{str(localIP.ip)}:{PORT}")

            logger.debug(f"Backend redirect IP/URL: {localServer}")
            return localServer
        return None

    def get_wifi_networks() -> Optional[list[str]]:
        ssids = []
        for s in WifiManager.scanForNetworks():
            ssids.append(s.ssid)
        return ssids

    def read_request(characteristic: BlessGATTCharacteristic, **kwargs) -> bytearray:
        try:
            improv_char = ImprovUUID(characteristic.uuid)
            logger.info(f"Reading {improv_char}")
        except Exception:
            logger.info(f"Reading {characteristic.uuid}")
            pass
        if characteristic.service_uuid == ImprovUUID.SERVICE_UUID.value:
            value = GATTServer.getServer().improv_server.handle_read(
                characteristic.uuid
            )
            GATTServer.getServer().allow_wifi_provisioning()
            return value
        return characteristic.value

    def write_request(
        characteristic: BlessGATTCharacteristic, value: bytearray, **kwargs
    ):
        if characteristic.service_uuid == ImprovUUID.SERVICE_UUID.value:
            (
                target_uuid,
                target_values,
            ) = GATTServer.getServer().improv_server.handle_write(
                characteristic.uuid, value
            )
            if target_uuid != None and target_values != None:
                for value in target_values:
                    logger.debug(f"Setting {ImprovUUID(target_uuid)} to {value}")
                    GATTServer.getServer().bless_gatt_server.get_characteristic(
                        target_uuid,
                    ).value = value
                    success = GATTServer.getServer().bless_gatt_server.update_value(
                        ImprovUUID.SERVICE_UUID.value, target_uuid
                    )
                    if not success:
                        logger.warning(
                            f"Updating characteristic return status={success}"
                        )
