from enum import Enum


class SegmentTopologySummaryType0Type(str, Enum):
    STRIP = "strip"

    def __str__(self) -> str:
        return str(self.value)
