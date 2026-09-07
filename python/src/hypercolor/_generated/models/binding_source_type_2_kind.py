from enum import Enum


class BindingSourceType2Kind(str, Enum):
    TIME = "time"

    def __str__(self) -> str:
        return str(self.value)
