from enum import Enum


class ApplyImpactType6(str, Enum):
    HARDWARE_PERSIST = "hardware_persist"

    def __str__(self) -> str:
        return str(self.value)
