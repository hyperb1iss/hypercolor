from ..tolerant_enum import TolerantStrEnum


class ControlKindType0(TolerantStrEnum):
    AREA = "area"
    BOOLEAN = "boolean"
    COLOR = "color"
    COMBOBOX = "combobox"
    HUE = "hue"
    NUMBER = "number"
    RECT = "rect"
    SENSOR = "sensor"
    TEXT = "text"

    def __str__(self) -> str:
        return str(self.value)
