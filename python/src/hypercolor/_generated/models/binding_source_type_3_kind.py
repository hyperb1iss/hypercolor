from enum import StrEnum


class BindingSourceType3Kind(StrEnum):
    CONSTANT = "constant"

    def __str__(self) -> str:
        return str(self.value)
