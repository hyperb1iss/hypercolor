from ..tolerant_enum import TolerantStrEnum


class EdgeBehaviorType0(TolerantStrEnum):
    CLAMP = "clamp"
    MIRROR = "mirror"
    WRAP = "wrap"

    def __str__(self) -> str:
        return str(self.value)
