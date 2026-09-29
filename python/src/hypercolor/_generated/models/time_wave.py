from enum import StrEnum


class TimeWave(StrEnum):
    SAW = "saw"
    SINE = "sine"
    SQUARE = "square"
    TRIANGLE = "triangle"

    def __str__(self) -> str:
        return str(self.value)
