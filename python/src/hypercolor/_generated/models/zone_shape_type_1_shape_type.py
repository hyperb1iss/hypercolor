from enum import StrEnum


class ZoneShapeType1ShapeType(StrEnum):
    ARC = "arc"

    def __str__(self) -> str:
        return str(self.value)
