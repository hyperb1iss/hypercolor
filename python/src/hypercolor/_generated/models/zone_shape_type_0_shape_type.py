from enum import Enum


class ZoneShapeType0ShapeType(str, Enum):
    RECTANGLE = "rectangle"

    def __str__(self) -> str:
        return str(self.value)
