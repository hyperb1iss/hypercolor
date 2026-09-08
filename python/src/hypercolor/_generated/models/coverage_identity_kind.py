from enum import Enum


class CoverageIdentityKind(str, Enum):
    DEVICE = "device"
    SERIAL = "serial"
    SMBUS = "smbus"
    USB_PATH = "usb_path"

    def __str__(self) -> str:
        return str(self.value)
