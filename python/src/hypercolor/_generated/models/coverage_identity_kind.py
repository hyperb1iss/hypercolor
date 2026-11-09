from ..tolerant_enum import TolerantStrEnum


class CoverageIdentityKind(TolerantStrEnum):
    DEVICE = "device"
    SERIAL = "serial"
    SMBUS = "smbus"
    USB_PATH = "usb_path"

    def __str__(self) -> str:
        return str(self.value)
