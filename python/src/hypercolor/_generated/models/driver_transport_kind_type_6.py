from enum import StrEnum


class DriverTransportKindType6(StrEnum):
    BRIDGE = "bridge"

    def __str__(self) -> str:
        return str(self.value)
