from enum import StrEnum


class ComponentCategoryType6(StrEnum):
    RADIATOR = "Radiator"

    def __str__(self) -> str:
        return str(self.value)
