from enum import Enum


class DisplayShape(str, Enum):
    ROUND = "round"
    SQUARE = "square"
    TALL = "tall"
    WIDE = "wide"

    def __str__(self) -> str:
        return str(self.value)
