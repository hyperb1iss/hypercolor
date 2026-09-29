from enum import StrEnum


class ComponentOrigin(StrEnum):
    BUILT_IN = "built_in"
    USER = "user"

    def __str__(self) -> str:
        return str(self.value)
