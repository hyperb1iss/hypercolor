from enum import StrEnum


class ControlKindType4(StrEnum):
    SENSOR = "sensor"

    def __str__(self) -> str:
        return str(self.value)
