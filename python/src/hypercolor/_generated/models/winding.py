from enum import StrEnum


class Winding(StrEnum):
    CLOCKWISE = "clockwise"
    COUNTER_CLOCKWISE = "counter_clockwise"

    def __str__(self) -> str:
        return str(self.value)
