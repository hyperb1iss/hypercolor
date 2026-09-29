from enum import StrEnum


class ComponentCategoryType8(StrEnum):
    RING = "Ring"

    def __str__(self) -> str:
        return str(self.value)
