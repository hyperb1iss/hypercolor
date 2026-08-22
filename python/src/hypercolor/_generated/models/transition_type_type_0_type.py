from enum import Enum


class TransitionTypeType0Type(str, Enum):
    CUT = "cut"

    def __str__(self) -> str:
        return str(self.value)
