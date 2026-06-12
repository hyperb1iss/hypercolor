from enum import Enum


class LedTopologyType1Type(str, Enum):
    MATRIX = "matrix"

    def __str__(self) -> str:
        return str(self.value)
