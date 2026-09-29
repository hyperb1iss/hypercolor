from enum import StrEnum


class BindingSourceType2Kind(StrEnum):
    TIME = "time"

    def __str__(self) -> str:
        return str(self.value)
