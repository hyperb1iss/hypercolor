from enum import Enum


class ZoneShapeType2ShapeType(str, Enum):
    RING = "ring"

    def __str__(self) -> str:
        return str(self.value)
