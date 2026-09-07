from enum import Enum


class ComponentCategoryType2(str, Enum):
    AIO = "Aio"

    def __str__(self) -> str:
        return str(self.value)
