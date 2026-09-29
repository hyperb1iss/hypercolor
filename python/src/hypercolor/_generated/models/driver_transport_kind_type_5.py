from enum import StrEnum


class DriverTransportKindType5(StrEnum):
    VIRTUAL = "virtual"

    def __str__(self) -> str:
        return str(self.value)
