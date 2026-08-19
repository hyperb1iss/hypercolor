from enum import Enum


class SegmentTopologySummaryType1Type(str, Enum):
    MATRIX = "matrix"

    def __str__(self) -> str:
        return str(self.value)
