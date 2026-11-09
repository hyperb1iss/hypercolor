from ..tolerant_enum import TolerantStrEnum


class Orientation(TolerantStrEnum):
    DIAGONAL = "diagonal"
    HORIZONTAL = "horizontal"
    RADIAL = "radial"
    VERTICAL = "vertical"

    def __str__(self) -> str:
        return str(self.value)
