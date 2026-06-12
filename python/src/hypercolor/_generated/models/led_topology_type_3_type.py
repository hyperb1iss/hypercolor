from enum import Enum


class LedTopologyType3Type(str, Enum):
    CONCENTRIC_RINGS = "concentric_rings"

    def __str__(self) -> str:
        return str(self.value)
