from enum import Enum


class SamplingModeType1Type(str, Enum):
    BILINEAR = "bilinear"

    def __str__(self) -> str:
        return str(self.value)
