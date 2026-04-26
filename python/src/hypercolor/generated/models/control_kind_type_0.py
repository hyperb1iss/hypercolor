from enum import Enum


class ControlKindType0(str, Enum):
    NUMBER = "number"

    def __str__(self) -> str:
        return str(self.value)
