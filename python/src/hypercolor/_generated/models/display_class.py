from ..tolerant_enum import TolerantStrEnum


class DisplayClass(TolerantStrEnum):
    PANEL = "panel"
    PUMP_LCD = "pump_lcd"
    STRIP = "strip"

    def __str__(self) -> str:
        return str(self.value)
