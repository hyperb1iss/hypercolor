from enum import Enum


class ComponentCategoryType4(str, Enum):
    CASE = "Case"

    def __str__(self) -> str:
        return str(self.value)
