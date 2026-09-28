from enum import StrEnum


class SamplingModeType2Type(StrEnum):
    AREA_AVERAGE = "area_average"

    def __str__(self) -> str:
        return str(self.value)
