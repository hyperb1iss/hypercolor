from ..tolerant_enum import TolerantStrEnum


class TimeWave(TolerantStrEnum):
    SAW = "saw"
    SINE = "sine"
    SQUARE = "square"
    TRIANGLE = "triangle"

    def __str__(self) -> str:
        return str(self.value)
