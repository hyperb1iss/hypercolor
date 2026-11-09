from ..tolerant_enum import TolerantStrEnum


class DisplayShape(TolerantStrEnum):
    ROUND = "round"
    SQUARE = "square"
    TALL = "tall"
    WIDE = "wide"

    def __str__(self) -> str:
        return str(self.value)
