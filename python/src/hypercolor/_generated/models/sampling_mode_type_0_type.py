from enum import Enum


class SamplingModeType0Type(str, Enum):
    NEAREST = "nearest"

    def __str__(self) -> str:
        return str(self.value)
