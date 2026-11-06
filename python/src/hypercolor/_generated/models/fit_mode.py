from ..tolerant_enum import TolerantStrEnum


class FitMode(TolerantStrEnum):
    CONTAIN = "contain"
    COVER = "cover"
    MIRROR = "mirror"
    STRETCH = "stretch"
    TILE = "tile"

    def __str__(self) -> str:
        return str(self.value)
