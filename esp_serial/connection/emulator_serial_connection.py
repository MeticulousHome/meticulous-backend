from .serial_connection import SerialConnection
from unittest.mock import MagicMock
from named_thread import NamedThread
import time
import os
import pty
import fcntl
from .emulation_data import EmulationData

from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)

EMULATION_SPEED = int(os.getenv("EMULATION_SPEED", "100"))


class EmulatorSerialConnection(SerialConnection):
    # Data and Sensor message every 150ms, therefore 75ms sleep per message
    DEFAULT_SLEEP_TIME = 0.075

    # Uses the parent constructor to connect to virtual serial
    def __init__(self) -> None:
        # Create virtual serial console (pty) for the backend to attach to
        self.them, self.us = pty.openpty()

        super().__init__(os.ttyname(self.us))
        self.line_counter = 0
        self.flasher = MagicMock()
        EmulationData.init()
        self.send_data_thread = NamedThread("EmulationData", target=self.send_data)
        self.send_data_thread.start()

    def send_data(self):
        logger.info("Setting emulation pty to non-blocking")
        flags = fcntl.fcntl(self.them, fcntl.F_GETFL)
        flags = flags | os.O_NONBLOCK
        fcntl.fcntl(self.them, fcntl.F_SETFL, flags)

        logger.info("Emulation thread started, sleeping 2 seconds")
        time.sleep(2.0)
        data_source = EmulationData.IDLE_DATA
        sleep_time = EmulatorSerialConnection.DEFAULT_SLEEP_TIME / (
            EMULATION_SPEED / 100.0
        )
        while True:
            line = ""

            if self.line_counter >= len(data_source) - 1:
                if data_source is not EmulationData.IDLE_DATA:
                    logger.info("Starting idle simulation!")
                self.line_counter = 0
                data_source = EmulationData.IDLE_DATA

            # As we set it to non-blocking read, it can immediatly return
            # with an exception
            try:
                host_commands = os.read(self.them, 2048)
                if b"action,start" in host_commands:
                    logger.info("Starting espresso simulation!")
                    data_source = EmulationData.ESPRESSO_DATA
                    self.line_counter = 0
                    continue
                if b"action,stop" in host_commands:
                    logger.info("Stopping all simulations, returning to idle!")
                    data_source = EmulationData.IDLE_DATA
                    self.line_counter = 0
                    continue
                if b"action,purge" in host_commands:
                    logger.info("Starting purge simulation!")
                    data_source = EmulationData.PURGE_DATA
                    self.line_counter = 0
                    continue
                if b"action,home" in host_commands:
                    logger.info("Starting purge simulation!")
                    data_source = EmulationData.HOME_DATA
                    self.line_counter = 0
                    continue
            except BlockingIOError:
                pass

            line = data_source[self.line_counter]
            self.line_counter += 1

            line = line.strip(" \t\r\n")

            if line == "":
                continue

            line += "\r\n"
            os.write(self.them, bytes(line.encode()))
            time.sleep(sleep_time)

    def reset(self, bootloader=False, sleep=0, ignored_bootloader_sleep=0):
        logger.info("Resetting Dummy ESP32")
        # Next iteration we will return to the idle mode with this
        self.line_counter = 1 << 42

    def sendUpdate(self):
        logger.info("Emulated ESP32 cannot be updated")
        return None
