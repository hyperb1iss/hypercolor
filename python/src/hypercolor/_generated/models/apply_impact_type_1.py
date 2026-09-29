from enum import StrEnum


class ApplyImpactType1(StrEnum):
    LIVE = "live"

    def __str__(self) -> str:
        return str(self.value)
