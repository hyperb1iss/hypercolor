from enum import Enum


class DriverTransportKindType0(str, Enum):
    NETWORK = "network"

    def __str__(self) -> str:
        return str(self.value)
