from config import CONFIG_USER, USB_MODE, USB_MODES, MeticulousConfig

from log import MeticulousLogger
from machine import Machine
from PTN5150 import PTN5150H, PortState, RpSelection

logger = MeticulousLogger.getLogger(__name__)


class USBManager:
    usbPdController = None

    @staticmethod
    def init():
        if Machine.emulated:
            return
        USBManager.usbPdController = PTN5150H()
        logger.info("USB PD Controller initialized")
        USBManager.setUSBMode()

    @staticmethod
    def setUSBMode():
        logger.warning("Setting USB mode is temporarely disabled")
        return
        if Machine.emulated:
            logger.info("Emulated machine, skipping USB mode setup")
            return
        ptn = USBManager.usbPdController

        match MeticulousConfig[CONFIG_USER][USB_MODE]:
            case USB_MODES.CLIENT.value:
                logger.info("Setting USB mode to CLIENT")
                ptn.set_port_state(PortState.UFP)
            case USB_MODES.HOST.value:
                logger.info("Setting USB mode to HOST")
                ptn.set_port_state(PortState.DFP)
            case USB_MODES.DUAL.value:
                logger.info("Setting USB mode to DUAL")
                ptn.set_port_state(PortState.DRP)

            case _:
                logger.error(
                    f"Unknown USB mode {MeticulousConfig[CONFIG_USER][USB_MODE]}"
                )
        ptn.set_rp_selection(RpSelection.X330_uA)
