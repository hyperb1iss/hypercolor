from enum import Enum


class ComponentCategoryType9(str, Enum):
    BULB = "Bulb"

    def __str__(self) -> str:
        return str(self.value)
