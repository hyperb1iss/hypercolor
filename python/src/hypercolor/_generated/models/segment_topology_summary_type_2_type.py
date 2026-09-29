from enum import StrEnum


class SegmentTopologySummaryType2Type(StrEnum):
    RING = "ring"

    def __str__(self) -> str:
        return str(self.value)
