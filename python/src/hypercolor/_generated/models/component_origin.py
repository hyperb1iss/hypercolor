from enum import Enum


class ComponentOrigin(str, Enum):
    BUILT_IN = "built_in"
    USER = "user"

    def __str__(self) -> str:
        return str(self.value)
