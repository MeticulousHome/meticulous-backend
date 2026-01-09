import logging
import os
import sys
from enum import Enum, auto
from typing import Optional

import esptool
import esptool.targets

from log import MeticulousLogger
from esptool.bin_image import LoadFirmwareImage
import struct

UPDATE_PATH = os.getenv("UPDATE_PATH", "/opt/meticulous-firmware")

"""
ESPTool uses print() to log to stdout.
We create our own logger which we can register as stdout pointer and therefore
redirect stdout to the logs.
"""
logger = MeticulousLogger.getLogger(__name__)


class FikaSupportedESP32(Enum):
    ESP32S1 = auto()
    ESP32S3 = auto()

    def fromString(chip: str) -> Optional["FikaSupportedESP32"]:
        if chip.upper() in ["ESP32", "ESP32S1", "ESP32-S1"]:
            return FikaSupportedESP32.ESP32S1
        if chip.upper() in ["ESP32S3", "ESP32-S3"]:
            return FikaSupportedESP32.ESP32S3
        return None


class ESPToolLogger:
    def __init__(self, level=logging.INFO):
        self.level = level

    def write(self, msg):
        if msg:
            msg = msg.strip()
            if len(msg) > 0:
                logger.log(self.level, msg)

    def flush(self):
        pass

    def isatty(self):
        return False


esp_tool_logger = ESPToolLogger()


class ESPToolArgs:
    default_esp32_s1_flash_mapping = [
        [0x1000, "bootloader.bin"],
        [0x8000, "partitions.bin"],
        [0xE000, "boot_app0.bin"],
        [0x10000, "firmware.bin"],
    ]

    default_esp32_s3_flash_mapping = [
        [0x0000, "bootloader.bin"],
        [0x8000, "partitions.bin"],
        [0xE000, "boot_app0.bin"],
        [0x10000, "firmware.bin"],
    ]

    def __init__(self):
        self.compress = True
        self.no_compress = False
        self.flash_size = "detect"
        self.flash_mode = "dio"
        self.flash_freq = "80m"
        self.no_stub = False
        self.verify = False
        self.erase_all = False
        self.encrypt = False
        self.encrypt_files = None
        self.no_progress = False
        self.addr_filename = []
        self.force = False

    def loadFlashMapping(self, chip: FikaSupportedESP32, mapping):
        firmware_path = ESPToolWrapper.getFirmwarePath(chip)
        if chip == FikaSupportedESP32.ESP32S1:
            self.chip = "esp32"
            # We compile the image for 8MB so we flash for that as well
            # This is wasting flash on mot chips but we are only using 1MB for app0 anyways :/
            self.flash_size = "8MB"
        elif chip == FikaSupportedESP32.ESP32S3:
            self.chip = "esp32s3"

        try:
            self.addr_filename = [
                [partition[0], open(os.path.join(firmware_path, partition[1]), "rb")]
                for partition in mapping
            ]
        except FileNotFoundError as e:
            logger.error(f"Failed to load firmware from disk: {e}")


class ESPToolWrapper:
    CONFIG_BAUD = 921600

    @staticmethod
    def getFirmwarePath(CHIP: FikaSupportedESP32):
        if CHIP == FikaSupportedESP32.ESP32S3:
            return os.path.join(UPDATE_PATH, "esp32-s3")
        elif CHIP == FikaSupportedESP32.ESP32S1:
            return os.path.join(UPDATE_PATH, "esp32")
        else:
            logger.error("Invalid CHIP for flash mapping!")
            return None

    def flash(self, port, reset=False):
        failure = None
        _stdout = sys.stdout
        logger.debug("Calling into esptool.py v%s" % esptool.__version__)
        if port is None:
            logger.warning("Running without esp32, not flashing!")
            return
        sys.stdout = esp_tool_logger

        # ESP32S3 is a superset of the esp32
        # So original esp32 will complain for unsupported features
        esp = esptool.targets.ESP32S3ROM(port)

        if reset:
            esp.connect("default_reset", 10, detecting=True)
        else:
            esp.connect("no_reset", 10, detecting=True)

        logger.info("Detecting chip type...")

        try:
            chip_id = esp.get_chip_id()
            logger.info(f"ChipID is {chip_id}")
            if chip_id != 9:
                logger.info("unexpected chip id")
            CHIP = FikaSupportedESP32.ESP32S3
            esp = esptool.targets.ESP32S3ROM(port)
        except esptool.util.UnsupportedCommandError:
            logger.info("Failed to get ESP32 ChipID, assuming ESP32-S1")
            CHIP = FikaSupportedESP32.ESP32S1
            esp = esptool.targets.ESP32ROM(port)
            pass

        # esp32 chip description is ESP32-D0WD-V3
        # esp32s3 chip is ESP32-D0WDQ6
        logger.info("Chip is %s" % (esp.get_chip_description()))
        logger.info("Features: %s" % ", ".join(esp.get_chip_features()))
        esptool.read_mac(esp, {})

        esp = esp.run_stub()

        initial_baudrate = port.baudrate

        try:
            logger.info(
                f"Changing baud from {initial_baudrate} to {ESPToolWrapper.CONFIG_BAUD}"
            )
            esp.change_baud(ESPToolWrapper.CONFIG_BAUD)
        except esptool.NotImplementedInROMError:
            logger.warning("ROM doesn't support changing baud rate. ")

        args = ESPToolArgs()
        # If we end up supporting  more chips this should be converted to a match, for now this is easier
        if CHIP == FikaSupportedESP32.ESP32S3:
            args.loadFlashMapping(CHIP, ESPToolArgs.default_esp32_s3_flash_mapping)
        elif CHIP == FikaSupportedESP32.ESP32S1:
            args.loadFlashMapping(CHIP, ESPToolArgs.default_esp32_s1_flash_mapping)
        else:
            logger.error("Invalid CHIP for flash mapping!")
            return None

        logger.info("Detecting flash")
        esptool.detect_flash_size(esp, args)
        logger.info(f"flash_size = {args.flash_size}")
        # Only needed for secure download mode
        esp.flash_set_parameters(esptool.flash_size_bytes(args.flash_size))

        try:
            esptool.write_flash(esp, args)
        except Exception as e:
            logger.error(f"Failed to flash: {e}")
            failure = e
        sys.stdout = _stdout

        logger.info("Done, Resetting baudrate")
        port.baudrate = initial_baudrate
        return failure

    def get_version_from_firmware():
        # ESP32-S3 is the default for customer hardware, everything else will be custom anyways
        try:
            image = LoadFirmwareImage(
                "esp32s3", os.path.join(UPDATE_PATH, "esp32-s3", "firmware.bin")
            )
        except Exception:
            return None

        app_desc = None
        for idx, seg in enumerate(image.segments, start=1):
            segs = seg.get_memory_type(image)
            if "DROM" in segs:
                app_desc = seg.data[:256]

        if app_desc:
            APP_DESC_STRUCT_FMT = "<II" + "8s" + "32s32s16s16s32s32s" + "80s"
            (
                magic_word,
                secure_version,
                reserv1,
                version,
                project_name,
                time,
                date,
                idf_ver,
                app_elf_sha256,
                reserv2,
            ) = struct.unpack(APP_DESC_STRUCT_FMT, app_desc)

            if magic_word == 0xABCD5432:
                logger.info("On-disk firmware file:")
                logger.info(f'Project name: {project_name.decode("utf-8").strip()}')
                logger.info(f'App version: {version.decode("utf-8").strip()}')
                logger.info(
                    f'Compile time: {date.decode("utf-8").strip()} {time.decode("utf-8").strip()}'
                )
                logger.info(f'ESP-IDF: {idf_ver.decode("utf-8").strip()}')
                logger.info(f"Secure version: {secure_version}")
                return version.decode("utf-8").strip(" \x00\t")
        return None
