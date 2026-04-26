from enum import Enum


class DriverTransportKindType5(str, Enum):
    VIRTUAL = "virtual"

    def __str__(self) -> str:
        return str(self.value)
