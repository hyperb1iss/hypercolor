from enum import StrEnum


class ControlKindType7(StrEnum):
    TEXT = "text"

    def __str__(self) -> str:
        return str(self.value)
