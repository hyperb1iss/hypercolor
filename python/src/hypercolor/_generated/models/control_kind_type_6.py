from enum import Enum


class ControlKindType6(str, Enum):
    AREA = "area"

    def __str__(self) -> str:
        return str(self.value)
