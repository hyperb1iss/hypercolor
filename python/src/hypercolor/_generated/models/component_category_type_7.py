from enum import StrEnum


class ComponentCategoryType7(StrEnum):
    MATRIX = "Matrix"

    def __str__(self) -> str:
        return str(self.value)
