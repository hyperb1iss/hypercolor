from enum import StrEnum


class LedTopologyType4Type(StrEnum):
    PERIMETER_LOOP = "perimeter_loop"

    def __str__(self) -> str:
        return str(self.value)
