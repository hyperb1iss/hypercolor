from enum import Enum


class DisplayClass(str, Enum):
    PANEL = "panel"
    PUMP_LCD = "pump_lcd"
    STRIP = "strip"

    def __str__(self) -> str:
        return str(self.value)
