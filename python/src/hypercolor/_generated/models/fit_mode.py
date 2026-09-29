from enum import StrEnum


class FitMode(StrEnum):
    CONTAIN = "contain"
    COVER = "cover"
    MIRROR = "mirror"
    STRETCH = "stretch"
    TILE = "tile"

    def __str__(self) -> str:
        return str(self.value)
