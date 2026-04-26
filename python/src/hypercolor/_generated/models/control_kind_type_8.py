from enum import Enum


class ControlKindType8(str, Enum):
    RECT = "rect"

    def __str__(self) -> str:
        return str(self.value)
