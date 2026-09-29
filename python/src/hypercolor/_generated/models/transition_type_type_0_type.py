from enum import StrEnum


class TransitionTypeType0Type(StrEnum):
    CUT = "cut"

    def __str__(self) -> str:
        return str(self.value)
