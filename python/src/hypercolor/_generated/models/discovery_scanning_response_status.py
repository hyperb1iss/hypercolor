from enum import StrEnum


class DiscoveryScanningResponseStatus(StrEnum):
    SCANNING = "scanning"

    def __str__(self) -> str:
        return str(self.value)
