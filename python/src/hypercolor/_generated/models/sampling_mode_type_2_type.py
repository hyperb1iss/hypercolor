from enum import Enum


class SamplingModeType2Type(str, Enum):
    AREA_AVERAGE = "area_average"

    def __str__(self) -> str:
        return str(self.value)
