from enum import StrEnum


class DisplayShape(StrEnum):
    ROUND = "round"
    SQUARE = "square"
    TALL = "tall"
    WIDE = "wide"

    def __str__(self) -> str:
        return str(self.value)
