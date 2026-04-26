from enum import Enum


class ControlKindType5(str, Enum):
    HUE = "hue"

    def __str__(self) -> str:
        return str(self.value)
