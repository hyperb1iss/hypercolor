from enum import StrEnum


class ZoneShapeType0ShapeType(StrEnum):
    RECTANGLE = "rectangle"

    def __str__(self) -> str:
        return str(self.value)
