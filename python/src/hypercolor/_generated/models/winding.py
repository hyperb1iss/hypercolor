from ..tolerant_enum import TolerantStrEnum


class Winding(TolerantStrEnum):
    CLOCKWISE = "clockwise"
    COUNTER_CLOCKWISE = "counter_clockwise"

    def __str__(self) -> str:
        return str(self.value)
