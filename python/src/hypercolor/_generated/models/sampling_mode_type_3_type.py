from enum import StrEnum


class SamplingModeType3Type(StrEnum):
    GAUSSIAN_AREA = "gaussian_area"

    def __str__(self) -> str:
        return str(self.value)
