from enum import StrEnum


class ControlKindType2(StrEnum):
    COLOR = "color"

    def __str__(self) -> str:
        return str(self.value)
