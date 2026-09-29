from enum import StrEnum


class Orientation(StrEnum):
    DIAGONAL = "diagonal"
    HORIZONTAL = "horizontal"
    RADIAL = "radial"
    VERTICAL = "vertical"

    def __str__(self) -> str:
        return str(self.value)
