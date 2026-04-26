from enum import Enum


class ZoneTopologySummaryType4Type(str, Enum):
    DISPLAY = "display"

    def __str__(self) -> str:
        return str(self.value)
