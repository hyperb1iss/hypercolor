from enum import Enum


class DiscoveryScanningResponseStatus(str, Enum):
    SCANNING = "scanning"

    def __str__(self) -> str:
        return str(self.value)
