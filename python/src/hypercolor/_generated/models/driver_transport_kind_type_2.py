from enum import StrEnum


class DriverTransportKindType2(StrEnum):
    SMBUS = "smbus"

    def __str__(self) -> str:
        return str(self.value)
