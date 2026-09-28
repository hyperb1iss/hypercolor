from enum import StrEnum


class LedTopologyType5Type(StrEnum):
    POINT = "point"

    def __str__(self) -> str:
        return str(self.value)
