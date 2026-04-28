from enum import Enum


class ApplyImpactType4(str, Enum):
    BACKEND_REBIND = "backend_rebind"

    def __str__(self) -> str:
        return str(self.value)
