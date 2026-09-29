from enum import StrEnum


class LedTopologyType3Type(StrEnum):
    CONCENTRIC_RINGS = "concentric_rings"

    def __str__(self) -> str:
        return str(self.value)
