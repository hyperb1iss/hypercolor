from enum import StrEnum


class EdgeBehaviorType0(StrEnum):
    CLAMP = "clamp"
    MIRROR = "mirror"
    WRAP = "wrap"

    def __str__(self) -> str:
        return str(self.value)
