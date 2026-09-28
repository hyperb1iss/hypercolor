from enum import StrEnum


class ZoneShapeType3ShapeType(StrEnum):
    CUSTOM = "custom"

    def __str__(self) -> str:
        return str(self.value)
