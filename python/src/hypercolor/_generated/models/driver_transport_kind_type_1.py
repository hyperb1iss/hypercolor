from enum import Enum


class DriverTransportKindType1(str, Enum):
    USB = "usb"

    def __str__(self) -> str:
        return str(self.value)
