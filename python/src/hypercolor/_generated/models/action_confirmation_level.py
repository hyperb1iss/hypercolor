from enum import StrEnum


class ActionConfirmationLevel(StrEnum):
    DESTRUCTIVE = "destructive"
    HARDWARE_PERSISTENT = "hardware_persistent"
    NORMAL = "normal"

    def __str__(self) -> str:
        return str(self.value)
