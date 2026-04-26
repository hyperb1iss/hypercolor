from enum import Enum


class DriverTransportKindType4(str, Enum):
    SERIAL = "serial"

    def __str__(self) -> str:
        return str(self.value)
