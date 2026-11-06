from enum import StrEnum


class DriverStability(StrEnum):
    EXPERIMENTAL = "experimental"
    STABLE = "stable"

    def __str__(self) -> str:
        return str(self.value)
