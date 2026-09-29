from enum import StrEnum


class ControlKindType6(StrEnum):
    AREA = "area"

    def __str__(self) -> str:
        return str(self.value)
