from enum import Enum


class ControlKindType2(str, Enum):
    COLOR = "color"

    def __str__(self) -> str:
        return str(self.value)
