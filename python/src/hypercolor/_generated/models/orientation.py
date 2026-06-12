from enum import Enum


class Orientation(str, Enum):
    DIAGONAL = "diagonal"
    HORIZONTAL = "horizontal"
    RADIAL = "radial"
    VERTICAL = "vertical"

    def __str__(self) -> str:
        return str(self.value)
