from enum import Enum


class TimeWave(str, Enum):
    SAW = "saw"
    SINE = "sine"
    SQUARE = "square"
    TRIANGLE = "triangle"

    def __str__(self) -> str:
        return str(self.value)
