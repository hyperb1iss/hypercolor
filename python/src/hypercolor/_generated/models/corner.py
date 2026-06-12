from enum import Enum


class Corner(str, Enum):
    BOTTOM_LEFT = "bottom_left"
    BOTTOM_RIGHT = "bottom_right"
    TOP_LEFT = "top_left"
    TOP_RIGHT = "top_right"

    def __str__(self) -> str:
        return str(self.value)
