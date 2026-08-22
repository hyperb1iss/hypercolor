from enum import Enum


class MacosSelectionStateType0Type(str, Enum):
    NONE = "none"

    def __str__(self) -> str:
        return str(self.value)
