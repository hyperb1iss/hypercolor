from enum import Enum


class LedTopologyType0Type(str, Enum):
    STRIP = "strip"

    def __str__(self) -> str:
        return str(self.value)
