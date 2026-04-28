from enum import Enum


class ActionConfirmationLevel(str, Enum):
    DESTRUCTIVE = "destructive"
    HARDWARE_PERSISTENT = "hardware_persistent"
    NORMAL = "normal"

    def __str__(self) -> str:
        return str(self.value)
