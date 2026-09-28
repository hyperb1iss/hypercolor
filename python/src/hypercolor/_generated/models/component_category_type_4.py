from enum import StrEnum


class ComponentCategoryType4(StrEnum):
    CASE = "Case"

    def __str__(self) -> str:
        return str(self.value)
