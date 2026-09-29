from enum import StrEnum


class ZoneShapeType2ShapeType(StrEnum):
    RING = "ring"

    def __str__(self) -> str:
        return str(self.value)
