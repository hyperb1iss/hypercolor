from enum import Enum


class ZoneShapeType3ShapeType(str, Enum):
    CUSTOM = "custom"

    def __str__(self) -> str:
        return str(self.value)
