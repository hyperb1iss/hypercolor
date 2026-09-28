from enum import StrEnum


class ComponentCategoryType0(StrEnum):
    FAN = "Fan"

    def __str__(self) -> str:
        return str(self.value)
