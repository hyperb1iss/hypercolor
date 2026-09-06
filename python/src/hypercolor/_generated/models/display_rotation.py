from enum import Enum


class DisplayRotation(str, Enum):
    DEG0 = "deg0"
    DEG180 = "deg180"
    DEG270 = "deg270"
    DEG90 = "deg90"

    def __str__(self) -> str:
        return str(self.value)
