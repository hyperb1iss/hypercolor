from enum import Enum


class ControlKindType3(str, Enum):
    COMBOBOX = "combobox"

    def __str__(self) -> str:
        return str(self.value)
