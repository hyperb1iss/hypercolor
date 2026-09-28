from enum import StrEnum


class DisplayRotation(StrEnum):
    DEG0 = "deg0"
    DEG180 = "deg180"
    DEG270 = "deg270"
    DEG90 = "deg90"

    def __str__(self) -> str:
        return str(self.value)
