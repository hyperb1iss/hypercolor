from enum import Enum


class ComponentCategoryType6(str, Enum):
    RADIATOR = "Radiator"

    def __str__(self) -> str:
        return str(self.value)
