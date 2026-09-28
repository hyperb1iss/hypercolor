from enum import StrEnum


class ComponentCategoryType5(StrEnum):
    HEATSINK = "Heatsink"

    def __str__(self) -> str:
        return str(self.value)
