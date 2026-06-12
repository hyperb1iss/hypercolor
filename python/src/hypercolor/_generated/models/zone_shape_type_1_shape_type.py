from enum import Enum


class ZoneShapeType1ShapeType(str, Enum):
    ARC = "arc"

    def __str__(self) -> str:
        return str(self.value)
