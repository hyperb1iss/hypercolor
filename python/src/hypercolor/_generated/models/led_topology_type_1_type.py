from enum import StrEnum


class LedTopologyType1Type(StrEnum):
    MATRIX = "matrix"

    def __str__(self) -> str:
        return str(self.value)
