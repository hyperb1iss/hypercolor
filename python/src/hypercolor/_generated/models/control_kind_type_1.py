from enum import StrEnum


class ControlKindType1(StrEnum):
    BOOLEAN = "boolean"

    def __str__(self) -> str:
        return str(self.value)
