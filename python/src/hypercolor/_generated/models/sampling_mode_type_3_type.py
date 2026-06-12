from enum import Enum


class SamplingModeType3Type(str, Enum):
    GAUSSIAN_AREA = "gaussian_area"

    def __str__(self) -> str:
        return str(self.value)
