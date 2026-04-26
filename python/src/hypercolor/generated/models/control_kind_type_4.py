from enum import Enum


class ControlKindType4(str, Enum):
    SENSOR = "sensor"

    def __str__(self) -> str:
        return str(self.value)
