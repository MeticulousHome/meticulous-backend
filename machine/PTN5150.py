import subprocess
from enum import IntEnum
from pydantic import BaseModel

class RpSelection(IntEnum):
    """
    Bits [4:3] in the Control register (0x02), controlling Rp (in DFP mode).

    00 => 80μA
    01 => 180μA
    10 => 330μA
    11 => Reserved
    """
    X80_uA = 0
    X180_uA = 1
    X330_uA = 2
    RESERVED = 3


class PortState(IntEnum):
    """
    Bits [2:1] in the Control register (0x02), controlling PORT pin / Mode selection.

    00 => UFP
    01 => DFP
    10 => DRP
    11 => Reserved
    """
    UFP = 0
    DFP = 1
    DRP = 2
    RESERVED = 3


class InterruptMask(IntEnum):
    """
    Bit [0] in the Control register (0x02), controlling the attach/detach interrupt mask.

    0 => Does not mask interrupts
    1 => Masks interrupts
    """
    NOT_MASKED = 0
    MASKED = 1


class VBUSDetect(IntEnum):
    """
    Bit [7] in CC Status register (0x04).

    0 => VBUS not detected
    1 => VBUS detected
    """
    NOT_DETECTED = 0
    DETECTED = 1


class RpDetect(IntEnum):
    """
    Bits [6:5] in CC Status register (0x04), valid in UFP mode.

    00 => Standby
    01 => Rp = Std USB (default 500mA)
    10 => Rp = 1.5A
    11 => Rp = 3.0A
    """
    STANDBY = 0
    STD_USB = 1
    A_1_5 = 2
    A_3_0 = 3


class PortAttachment(IntEnum):
    """
    Bits [4:2] in CC Status register (0x04).

    000 => Not Connected
    001 => DFP attached
    010 => UFP attached
    011 => Analog Audio Accessory
    100 => Debug Accessory
    101 => Reserved
    110 => Reserved
    111 => Reserved
    """
    NOT_CONNECTED = 0
    DFP_ATTACHED = 1
    UFP_ATTACHED = 2
    ANALOG_AUDIO = 3
    DEBUG_ACCESSORY = 4
    RESERVED_5 = 5
    RESERVED_6 = 6
    RESERVED_7 = 7


class CCPolarity(IntEnum):
    """
    Bits [1:0] in CC Status register (0x04).

    00 => Cable Not Attached
    01 => CC1 is connected (normal orientation)
    10 => CC2 is connected (reversed orientation)
    11 => Reserved
    """
    NOT_ATTACHED = 0
    CC1_NORMAL = 1
    CC2_REVERSED = 2
    RESERVED = 3


#
# Optionally, a Pydantic model to hold CC status as a typed structure
#
class CCStatus(BaseModel):
    vbus_detect: VBUSDetect
    rp_detect: RpDetect
    port_attachment: PortAttachment
    cc_polarity: CCPolarity



class PTN5150H:
    """
    A simple class to access PTN5150H registers over I2C using i2cget/i2cset.
    Datasheet: https://web.archive.org/web/20240515225630/http://www.nxp.com/docs/en/data-sheet/PTN5150H.pdf
    """

    def __init__(self, i2cbus=1, address=0x3d):
        """
        :param i2cbus: The I2C bus number (e.g. 1 on a Raspberry Pi).
        :param address: The 7-bit address of the PTN5150H (default = 0x3D).
        """
        self.i2cbus = i2cbus
        self.address = address

    def read_register(self, reg_addr: int) -> int:
        """
        Reads an 8-bit value from the given register address using i2cget.
        :param reg_addr: The register address to read (0x00..0xFF).
        :return: The register value (0..255).
        """
        cmd = [
            "i2cget",
            "-f"
            "-y", str(self.i2cbus),
            f"0x{self.address:02x}",
            f"0x{reg_addr:02x}"
        ]
        output = subprocess.check_output(cmd).strip()
        # i2cget outputs a string like "0x1f"
        return int(output, 16)

    def write_register(self, reg_addr: int, value: int) -> None:
        """
        Writes an 8-bit value to the given register address using i2cset.
        :param reg_addr: The register address to write (0x00..0xFF).
        :param value: The value to write (0..255).
        """
        cmd = [
            "i2cset",
            "-f"
            "-y", str(self.i2cbus),
            f"0x{self.address:02x}",
            f"0x{reg_addr:02x}",
            f"0x{value:02x}"
        ]
        subprocess.check_call(cmd)

    # ------------------------------------------------------------------------
    # Register 0x01: Version / Vendor (Read Only)
    #
    #   Bits [7:3] = Version ID (5 bits)
    #   Bits [2:0] = Vendor ID  (3 bits)
    # ------------------------------------------------------------------------
    def get_version_id(self) -> int:
        """
        Bits [7:3] = version ID
        """
        reg_val = self.read_register(0x01)
        version_id = (reg_val >> 3) & 0x1F
        return version_id

    def get_vendor_id(self) -> int:
        """
        Bits [2:0] = vendor ID
        """
        reg_val = self.read_register(0x01)
        vendor_id = reg_val & 0x07
        return vendor_id

    # ------------------------------------------------------------------------
    # Register 0x02: Control (Read/Write)
    #
    #   Bits [4:3] = Rp Selection
    #   Bits [2:1] = PORT pin state / Mode Selection
    #   Bit  [0]   = Interrupt Mask
    # ------------------------------------------------------------------------
    def get_control(self) -> int:
        """Returns the entire 8-bit control register (0x02)."""
        return self.read_register(0x02)

    def set_control(self, value: int) -> None:
        """Writes the entire 8-bit control register (0x02)."""
        self.write_register(0x02, value)

    #
    # Rp Selection
    #
    def get_rp_selection(self) -> RpSelection:
        """
        Extract bits [4:3] from the control register and return an RpSelection enum.
        """
        reg_val = self.read_register(0x02)
        raw = (reg_val >> 3) & 0x03
        return RpSelection(raw)

    def set_rp_selection(self, rp_sel: RpSelection) -> None:
        """
        Set bits [4:3] in control register to the given RpSelection enum.
        """
        reg_val = self.read_register(0x02)
        # Clear bits [4:3], then set them
        reg_val = (reg_val & 0xE7) | ((rp_sel.value & 0x03) << 3)
        self.write_register(0x02, reg_val)

    #
    # Port State (Mode Selection)
    #
    def get_port_state(self) -> PortState:
        """
        Extract bits [2:1] from the control register and return a PortState enum.
        """
        reg_val = self.read_register(0x02)
        raw = (reg_val >> 1) & 0x03
        return PortState(raw)

    def set_port_state(self, port_mode: PortState) -> None:
        """
        Set bits [2:1] in control register to the given PortState enum.
        """
        reg_val = self.read_register(0x02)
        # Clear bits [2:1], then set them
        reg_val = (reg_val & 0xF9) | ((port_mode.value & 0x03) << 1)
        self.write_register(0x02, reg_val)

    #
    # Interrupt Mask (bit [0])
    #
    def get_interrupt_mask(self) -> InterruptMask:
        """
        Extract bit [0] from the control register and return an InterruptMask enum.
        """
        reg_val = self.read_register(0x02)
        raw = reg_val & 0x01
        return InterruptMask(raw)

    def set_interrupt_mask(self, mask_bit: InterruptMask) -> None:
        """
        Set bit [0] in the control register to the given InterruptMask enum.
        """
        reg_val = self.read_register(0x02)
        reg_val = (reg_val & 0xFE) | (mask_bit.value & 0x01)
        self.write_register(0x02, reg_val)

    # ------------------------------------------------------------------------
    # Register 0x03: Interrupt Status (Read-Only, Clear on Read)
    #
    #   Bits [1] = Cable Detach Interrupt
    #   Bits [0] = Cable Attach Interrupt
    # ------------------------------------------------------------------------
    def get_interrupt_status(self) -> dict:
        """
        Reads the interrupt status at 0x03 (clears on read).
        Returns a dict with 'cable_detach' and 'cable_attach' flags (booleans).
        """
        reg_val = self.read_register(0x03)
        return {
            "cable_detach": bool((reg_val >> 1) & 0x01),
            "cable_attach": bool(reg_val & 0x01),
        }

    # ------------------------------------------------------------------------
    # Register 0x04: CC Status (Read-Only)
    #
    #   [7]   = VBUS Detection (0=not detected, 1=detected)
    #   [6:5] = Rp Detection (when UFP)
    #   [4:2] = Port Attachment Status
    #   [1:0] = CC Polarity
    # ------------------------------------------------------------------------
    def get_cc_status(self) -> CCStatus:
        """
        Reads and parses register 0x04
        """
        reg_val = self.read_register(0x04)

        vbus = VBUSDetect((reg_val >> 7) & 0x01)
        rp_d = RpDetect((reg_val >> 5) & 0x03)
        attach = PortAttachment((reg_val >> 2) & 0x07)
        polarity = CCPolarity(reg_val & 0x03)

        return CCStatus(
            vbus_detect=vbus,
            rp_detect=rp_d,
            port_attachment=attach,
            cc_polarity=polarity
        )


#
# Example usage:
#
if __name__ == "__main__":
    # Instantiate PTN5150H at bus=1, address=0x3D
    ptn = PTN5150H(i2cbus=1, address=0x3D)

    # Read vendor/version
    version = ptn.get_version_id()
    vendor = ptn.get_vendor_id()
    print("Version ID:", version)
    print("Vendor ID:", vendor)

    # Read and parse control register bitfields
    rp_sel = ptn.get_rp_selection()
    port_mode = ptn.get_port_state()
    intr_mask = ptn.get_interrupt_mask()
    print(f"Current RpSelection: {rp_sel.name} ({rp_sel.value})")
    print(f"Current PortState: {port_mode.name} ({port_mode.value})")
    print(f"Current InterruptMask: {intr_mask.name} ({intr_mask.value})")

    # Update some fields
    ptn.set_rp_selection(RpSelection.X180_uA)  # 180μA
    ptn.set_port_state(PortState.DRP)          # DRP
    ptn.set_interrupt_mask(InterruptMask.MASKED)

    # Check (and clear) interrupt status
    interrupts = ptn.get_interrupt_status()
    print("Interrupts:", interrupts)

    # Read CC status as a typed Pydantic model
    cc_stat = ptn.get_cc_status()
    print("CC Status model:", cc_stat.dict())
