from enum import Enum


class ComponentCategoryType0(str, Enum):
    FAN = "Fan"

    def __str__(self) -> str:
        return str(self.value)
