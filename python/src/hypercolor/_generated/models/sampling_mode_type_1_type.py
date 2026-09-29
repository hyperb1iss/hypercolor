from enum import StrEnum


class SamplingModeType1Type(StrEnum):
    BILINEAR = "bilinear"

    def __str__(self) -> str:
        return str(self.value)
