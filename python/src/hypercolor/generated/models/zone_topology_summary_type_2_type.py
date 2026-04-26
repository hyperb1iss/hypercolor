from enum import Enum


class ZoneTopologySummaryType2Type(str, Enum):
    RING = "ring"

    def __str__(self) -> str:
        return str(self.value)
