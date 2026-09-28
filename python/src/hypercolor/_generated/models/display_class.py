from enum import StrEnum


class DisplayClass(StrEnum):
    PANEL = "panel"
    PUMP_LCD = "pump_lcd"
    STRIP = "strip"

    def __str__(self) -> str:
        return str(self.value)
