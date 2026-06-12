from enum import Enum


class LedTopologyType5Type(str, Enum):
    POINT = "point"

    def __str__(self) -> str:
        return str(self.value)
