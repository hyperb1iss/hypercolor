from enum import StrEnum


class ControlKindType5(StrEnum):
    HUE = "hue"

    def __str__(self) -> str:
        return str(self.value)
