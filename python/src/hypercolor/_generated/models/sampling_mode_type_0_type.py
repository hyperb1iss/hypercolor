from enum import StrEnum


class SamplingModeType0Type(StrEnum):
    NEAREST = "nearest"

    def __str__(self) -> str:
        return str(self.value)
