from enum import StrEnum


class SegmentTopologySummaryType1Type(StrEnum):
    MATRIX = "matrix"

    def __str__(self) -> str:
        return str(self.value)
