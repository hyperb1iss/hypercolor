from ..tolerant_enum import TolerantStrEnum


class DriverStability(TolerantStrEnum):
    EXPERIMENTAL = "experimental"
    STABLE = "stable"

    def __str__(self) -> str:
        return str(self.value)
