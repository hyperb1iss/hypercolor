from enum import StrEnum


class ComponentCategoryType2(StrEnum):
    AIO = "Aio"

    def __str__(self) -> str:
        return str(self.value)
