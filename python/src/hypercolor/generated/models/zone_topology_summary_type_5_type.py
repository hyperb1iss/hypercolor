from enum import Enum


class ZoneTopologySummaryType5Type(str, Enum):
    CUSTOM = "custom"

    def __str__(self) -> str:
        return str(self.value)
