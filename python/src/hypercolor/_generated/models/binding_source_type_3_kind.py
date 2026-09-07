from enum import Enum


class BindingSourceType3Kind(str, Enum):
    CONSTANT = "constant"

    def __str__(self) -> str:
        return str(self.value)
