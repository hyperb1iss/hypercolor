from enum import StrEnum


class SegmentTopologySummaryType3Type(StrEnum):
    POINT = "point"

    def __str__(self) -> str:
        return str(self.value)
