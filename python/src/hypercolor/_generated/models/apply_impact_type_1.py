from enum import Enum


class ApplyImpactType1(str, Enum):
    LIVE = "live"

    def __str__(self) -> str:
        return str(self.value)
