from enum import StrEnum


class LedTopologyType0Type(StrEnum):
    STRIP = "strip"

    def __str__(self) -> str:
        return str(self.value)
