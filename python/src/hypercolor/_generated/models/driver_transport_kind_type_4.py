from enum import StrEnum


class DriverTransportKindType4(StrEnum):
    SERIAL = "serial"

    def __str__(self) -> str:
        return str(self.value)
