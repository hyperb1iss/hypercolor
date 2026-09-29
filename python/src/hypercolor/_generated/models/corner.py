from enum import StrEnum


class Corner(StrEnum):
    BOTTOM_LEFT = "bottom_left"
    BOTTOM_RIGHT = "bottom_right"
    TOP_LEFT = "top_left"
    TOP_RIGHT = "top_right"

    def __str__(self) -> str:
        return str(self.value)
