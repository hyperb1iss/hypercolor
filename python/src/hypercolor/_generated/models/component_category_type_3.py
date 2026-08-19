from enum import Enum


class ComponentCategoryType3(str, Enum):
    STRIMER = "Strimer"

    def __str__(self) -> str:
        return str(self.value)
