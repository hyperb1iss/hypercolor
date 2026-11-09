from ..tolerant_enum import TolerantStrEnum


class ComponentCategoryType0(TolerantStrEnum):
    AIO = "Aio"
    BULB = "Bulb"
    CASE = "Case"
    FAN = "Fan"
    HEATSINK = "Heatsink"
    MATRIX = "Matrix"
    RADIATOR = "Radiator"
    RING = "Ring"
    STRIMER = "Strimer"
    STRIP = "Strip"

    def __str__(self) -> str:
        return str(self.value)
