from enum import Enum


class ZoneTopologySummaryType3Type(str, Enum):
    POINT = "point"

    def __str__(self) -> str:
        return str(self.value)
