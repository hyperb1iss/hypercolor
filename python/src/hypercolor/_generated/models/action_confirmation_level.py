from ..tolerant_enum import TolerantStrEnum


class ActionConfirmationLevel(TolerantStrEnum):
    DESTRUCTIVE = "destructive"
    HARDWARE_PERSISTENT = "hardware_persistent"
    NORMAL = "normal"

    def __str__(self) -> str:
        return str(self.value)
