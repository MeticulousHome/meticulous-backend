import logging
import os
import sys

import esptool
from log import MeticulousLogger
from esptool.bin_image import LoadFirmwareImage
import struct

UPDATE_PATH = os.getenv("UPDATE_PATH", '/opt/meticulous-firmware')

"""
ESPTool uses print() to log to stdout.
We create our own logger which we can register as stdout pointer and therefore
redirect stdout to the logs.
"""
logger = MeticulousLogger.getLogger(__name__)
class ESPToolLogger:
    def __init__(self, level=logging.INFO):
        self.level = level

    def write(self, msg):
        if msg:
            msg = msg.strip()
            if len(msg) > 0:
                logger.log(self.level, msg)

    def flush(self): pass
    def isatty(self): return False

esp_tool_logger = ESPToolLogger()

class ESPToolArgs:
    default_flash_mapping = [
        [0x1000, "bootloader.bin"],
        [0xe000, "boot_app0.bin"],
        [0x8000, "partitions.bin"],
        [0x10000, "firmware.bin"]
    ]

    def __init__(self, chip="esp32"):
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
        self.chip = chip

    def loadFlashMapping(self, search_path=".", mapping=default_flash_mapping):
        self.addr_filename = [[partition[0], open(os.path.join(
            search_path, partition[1]), 'rb')] for partition in mapping]

class ESPToolWrapper():
    CONFIG_BAUD = 921600

    def flash(self, port, reset=False):
        _stdout = sys.stdout

        logger.debug("Calling into esptool.py v%s" % esptool.__version__)
        if port is None:
            logger.warning("Running without esp32, not flashing!")
            return
        sys.stdout = esp_tool_logger

        esp = esptool.ESP32ROM(port)
        if reset:
            esp.connect("default_reset", 10, detecting=True)
        else:
            esp.connect("no_reset", 10, detecting=True)

        logger.info("Detecting chip type...")

        try:
            chip_id = esp.get_chip_id()
            logger.info(f"ChipID is {chip_id}")
        except esptool.util.NotImplementedInROMError as e:
            pass

        logger.info("Chip is %s" % (esp.get_chip_description()))
        logger.info("Features: %s" % ", ".join(esp.get_chip_features()))
        esptool.read_mac(esp, {})

        esp = esp.run_stub()

        initial_baudrate = port.baudrate

        try:
            logger.info(
                f"Changing baud from {initial_baudrate} to {ESPToolWrapper.CONFIG_BAUD}")
            esp.change_baud(ESPToolWrapper.CONFIG_BAUD)
        except esptool.NotImplementedInROMError:
            logger.warning(
                "ROM doesn't support changing baud rate. "
            )

        args = ESPToolArgs()
        args.loadFlashMapping(UPDATE_PATH)

        logger.info("Detecting flash")
        esptool.detect_flash_size(esp, args)
        logger.info(f"flash_size = {args.flash_size}")
        # Only needed for secure download mode
        esp.flash_set_parameters(esptool.flash_size_bytes(args.flash_size))

        esptool.write_flash(esp, args)
        sys.stdout = _stdout

        logger.info("Done, Resetting baudrate")
        port.baudrate = initial_baudrate

    def get_version_from_firmware():
        try:
            image = LoadFirmwareImage("ESP32", os.path.join(UPDATE_PATH, "firmware.bin"))
        except:
            return None

        for idx, seg in enumerate(image.segments, start=1):
            segs = seg.get_memory_type(image)
            seg_name = ", ".join(segs)
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
                logger.info(f"On-disk firmware file:")
                logger.info(f'Project name: {project_name.decode("utf-8")}')
                logger.info(f'App version: {version.decode("utf-8")}')
                logger.info(f'Compile time: {date.decode("utf-8")} {time.decode("utf-8")}')
                logger.info(f'ESP-IDF: {idf_ver.decode("utf-8")}')
                logger.info(f"Secure version: {secure_version}")
                return version.decode("utf-8").strip(" \x00\t")
        return None
