from enum import StrEnum


class ComponentCategoryType3(StrEnum):
    STRIMER = "Strimer"

    def __str__(self) -> str:
        return str(self.value)
