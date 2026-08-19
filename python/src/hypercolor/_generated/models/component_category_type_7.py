from enum import Enum


class ComponentCategoryType7(str, Enum):
    MATRIX = "Matrix"

    def __str__(self) -> str:
        return str(self.value)
