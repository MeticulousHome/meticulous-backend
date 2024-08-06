import time
import gpiod
from .serial_connection import SerialConnection
import serial
import serial.tools.list_ports

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

try:
    if "serialization" in serial.__doc__ and "deserialization" in serial.__doc__:
        raise ImportError("The backend depends on pyserial which conflicts serial")
except TypeError:
    pass  # __doc__ returns None for pyserial


class FikaSerialConnection(SerialConnection):
    class PinConfig:
        def __init__(
            self,
            chip,
            pin,
            alias: str = None,
            direction=gpiod.line_request.DIRECTION_OUTPUT,
        ) -> None:
            self.chip = chip
            self.pin = pin
            self.alias = alias
            self.direction = direction

    # GPIO pin aliases
    ENABLE_PIN = "en"
    ESP_ENABLE_PIN = "esp_en"
    BOOT_PIN = "io0"
    BUFFER_PIN = "buffer_pin"

    # Fika V3+ pinconfig
    DEFAULT_PINS = [
        PinConfig(4, 9, ENABLE_PIN),
        PinConfig(0, 7, ESP_ENABLE_PIN),
        PinConfig(0, 8, BOOT_PIN),
        PinConfig(3, 26, BUFFER_PIN),
    ]

    def __init__(self, device, pins: [PinConfig] = DEFAULT_PINS) -> None:
        self.mapping = {
            FikaSerialConnection.ENABLE_PIN: None,
            FikaSerialConnection.ESP_ENABLE_PIN: None,
            FikaSerialConnection.BOOT_PIN: None,
            FikaSerialConnection.BUFFER_PIN: None,
        }
        self.pins = {}
        self._requestPins(pins)
        # SerialConnection connects on init, therefore we do it last
        super().__init__(device)

    def connect(self, device):
        logger.info("Connecting to Fika ESP32")
        if device is None:
            raise AttributeError("Fika board UART unspecified")
        return super().connect(device)

    def _requestPins(self, pins: [PinConfig] = DEFAULT_PINS) -> None:
        self.config = gpiod.line_request()
        self.config.consumer = __name__
        self.config.request_type = gpiod.line_request.DIRECTION_OUTPUT
        self.gpioChip = {}

        for pin in pins:
            if pin.chip not in self.gpioChip:
                self.gpioChip[pin.chip] = gpiod.chip(pin.chip)

            chip = self.gpioChip[pin.chip]
            self.pins[pin] = chip.get_line(pin.pin)
            self.pins[pin].request(self.config)
            self.mapping[pin.alias] = self.pins[pin]

    def _setOutputPin(self, alias, level):
        logger.debug(f"Setting pin {alias} to {level}")
        pin = self.mapping[alias]
        if pin is not None:
            pin.set_value(level)

    def _setBootPin(self, state):
        self._setOutputPin(FikaSerialConnection.BOOT_PIN, state)

    def _setResetPin(self, state):
        self._setOutputPin(FikaSerialConnection.ESP_ENABLE_PIN, state)

    def reset(self, bootloader=False, sleep=0.1, bootloader_sleep=0.1):
        logger.info(f"Resetting ESP32, bootloder = {bootloader}")

        self._setBootPin(0)
        # Put chip into reset
        self._setResetPin(1)
        time.sleep(sleep)
        self._setBootPin(int(bootloader))
        # Release Chip from reset
        self._setResetPin(0)

        if bootloader:
            time.sleep(bootloader_sleep)
            self._setBootPin(0)
