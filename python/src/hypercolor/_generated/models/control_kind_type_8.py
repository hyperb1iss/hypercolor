from enum import StrEnum


class ControlKindType8(StrEnum):
    RECT = "rect"

    def __str__(self) -> str:
        return str(self.value)
