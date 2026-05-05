from enum import Enum


class DriverTransportKindType6(str, Enum):
    BRIDGE = "bridge"

    def __str__(self) -> str:
        return str(self.value)
