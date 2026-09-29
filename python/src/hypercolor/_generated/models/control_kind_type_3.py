from enum import StrEnum


class ControlKindType3(StrEnum):
    COMBOBOX = "combobox"

    def __str__(self) -> str:
        return str(self.value)
