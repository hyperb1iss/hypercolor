from enum import Enum


class DriverTransportKindType2(str, Enum):
    SMBUS = "smbus"

    def __str__(self) -> str:
        return str(self.value)
