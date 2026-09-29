from enum import StrEnum


class ComponentCategoryType1(StrEnum):
    STRIP = "Strip"

    def __str__(self) -> str:
        return str(self.value)
