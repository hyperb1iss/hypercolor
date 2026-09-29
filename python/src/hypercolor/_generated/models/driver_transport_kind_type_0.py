from enum import StrEnum


class DriverTransportKindType0(StrEnum):
    NETWORK = "network"

    def __str__(self) -> str:
        return str(self.value)
