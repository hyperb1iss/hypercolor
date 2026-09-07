from enum import Enum


class FitMode(str, Enum):
    CONTAIN = "contain"
    COVER = "cover"
    MIRROR = "mirror"
    STRETCH = "stretch"
    TILE = "tile"

    def __str__(self) -> str:
        return str(self.value)
