from enum import Enum


class ComponentCategoryType1(str, Enum):
    STRIP = "Strip"

    def __str__(self) -> str:
        return str(self.value)
