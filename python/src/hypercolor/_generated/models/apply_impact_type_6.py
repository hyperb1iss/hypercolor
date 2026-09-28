from enum import StrEnum


class ApplyImpactType6(StrEnum):
    HARDWARE_PERSIST = "hardware_persist"

    def __str__(self) -> str:
        return str(self.value)
