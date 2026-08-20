from enum import Enum


class ComponentCategoryType5(str, Enum):
    HEATSINK = "Heatsink"

    def __str__(self) -> str:
        return str(self.value)
