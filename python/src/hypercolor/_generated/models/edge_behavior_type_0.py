from enum import Enum


class EdgeBehaviorType0(str, Enum):
    CLAMP = "clamp"
    MIRROR = "mirror"
    WRAP = "wrap"

    def __str__(self) -> str:
        return str(self.value)
