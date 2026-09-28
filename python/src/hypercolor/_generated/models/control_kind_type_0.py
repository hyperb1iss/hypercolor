from enum import StrEnum


class ControlKindType0(StrEnum):
    NUMBER = "number"

    def __str__(self) -> str:
        return str(self.value)
