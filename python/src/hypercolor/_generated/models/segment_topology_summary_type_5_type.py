from enum import Enum


class SegmentTopologySummaryType5Type(str, Enum):
    CUSTOM = "custom"

    def __str__(self) -> str:
        return str(self.value)
