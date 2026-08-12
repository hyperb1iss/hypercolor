from enum import Enum


class MacosSelectionStateApiType1Type(str, Enum):
    DISPLAY = "display"

    def __str__(self) -> str:
        return str(self.value)
