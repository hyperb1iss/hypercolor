from enum import StrEnum


class DriverTransportKindType1(StrEnum):
    USB = "usb"

    def __str__(self) -> str:
        return str(self.value)
