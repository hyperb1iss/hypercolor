from ..tolerant_enum import TolerantStrEnum


class DisplayRotation(TolerantStrEnum):
    DEG0 = "deg0"
    DEG180 = "deg180"
    DEG270 = "deg270"
    DEG90 = "deg90"

    def __str__(self) -> str:
        return str(self.value)
