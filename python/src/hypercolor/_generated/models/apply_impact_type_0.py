from enum import StrEnum


class ApplyImpactType0(StrEnum):
    NONE = "none"

    def __str__(self) -> str:
        return str(self.value)
