from ..tolerant_enum import TolerantStrEnum


class Corner(TolerantStrEnum):
    BOTTOM_LEFT = "bottom_left"
    BOTTOM_RIGHT = "bottom_right"
    TOP_LEFT = "top_left"
    TOP_RIGHT = "top_right"

    def __str__(self) -> str:
        return str(self.value)
