from ..tolerant_enum import TolerantStrEnum


class ComponentOrigin(TolerantStrEnum):
    BUILT_IN = "built_in"
    USER = "user"

    def __str__(self) -> str:
        return str(self.value)
