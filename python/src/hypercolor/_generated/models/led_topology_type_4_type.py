from enum import Enum


class LedTopologyType4Type(str, Enum):
    PERIMETER_LOOP = "perimeter_loop"

    def __str__(self) -> str:
        return str(self.value)
