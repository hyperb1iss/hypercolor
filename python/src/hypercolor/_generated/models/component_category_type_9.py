from enum import StrEnum


class ComponentCategoryType9(StrEnum):
    BULB = "Bulb"

    def __str__(self) -> str:
        return str(self.value)
