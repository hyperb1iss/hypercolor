from enum import Enum


class ComponentCategoryType8(str, Enum):
    RING = "Ring"

    def __str__(self) -> str:
        return str(self.value)
