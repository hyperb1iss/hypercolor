from enum import StrEnum


class ApplyImpactType4(StrEnum):
    BACKEND_REBIND = "backend_rebind"

    def __str__(self) -> str:
        return str(self.value)
