from enum import Enum


class Winding(str, Enum):
    CLOCKWISE = "clockwise"
    COUNTER_CLOCKWISE = "counter_clockwise"

    def __str__(self) -> str:
        return str(self.value)
