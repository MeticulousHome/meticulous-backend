from ..esp_tool_wrapper import ESPToolWrapper
import serial, serial.tools.list_ports

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)

try:
    if "serialization" in serial.__doc__ and "deserialization" in serial.__doc__:
        raise ImportError(
            "The backend depends on pyserial which conflicts serial"
        )
except TypeError:
    pass  # __doc__ returns None for pyserial

class SerialConnection():
    BAUDRATE = 115200
    CONNECT_TIMEOUT = 2

    def __init__(self, device) -> None:
        self.flasher = ESPToolWrapper()
        self.device = None
        self.port = None
        self.connected = False

        if self.connect(device) is None or not self.connected:
            raise RuntimeError("Couldn't connect to ESP32")

        logger.info("An ESP32 is connected")

    def available(self):
        return self.port != None and self.connected

    def connect(self, devices):
        if not isinstance(devices, list):
            devices = [devices]

        logger.debug(f"All serial devices to test: {devices}")
        for port in devices:
            logger.info(f"Testing connect to {port}");
            try:
                ser = serial.Serial(port, baudrate=SerialConnection.BAUDRATE, timeout=SerialConnection.CONNECT_TIMEOUT)
                # FIXME In back.py:
                # Read one byte or wait until timeout above is reached
                # This test assues an existing firmware
                self.device = port
                self.port = ser
                self.connected = True
                logger.info(f"Successfully connected to {port}");
                return port

            except (OSError, serial.SerialException) as e:
                logger.info(f"Serial Exception raised while trying to connect: {e}")
        # If no Device was detected, return None
        logger.warning("No serial Device found!")
        return None

    def disconnect(self):
        if self.port:
            self.port.close()

    def reset(**args):
        raise NotImplementedError()

    def sendUpdate(self):
        if self.port is None or not self.available():
            raise serial.PortNotOpenError("Cannot update firmare!")

        self.reset(bootloader=True)
        error_msg = self.flasher.flash(self.port, reset=False)

        # esptool cannot reset the fika board so we alwys reset manually
        self.reset()

        # Just to be sure
        self.port.baudrate = SerialConnection.BAUDRATE
        return error_msg
