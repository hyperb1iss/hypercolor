from enum import Enum


class ControlKindType1(str, Enum):
    BOOLEAN = "boolean"

    def __str__(self) -> str:
        return str(self.value)
