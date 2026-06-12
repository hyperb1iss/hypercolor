from enum import Enum


class StripDirection(str, Enum):
    BOTTOM_TO_TOP = "bottom_to_top"
    LEFT_TO_RIGHT = "left_to_right"
    RIGHT_TO_LEFT = "right_to_left"
    TOP_TO_BOTTOM = "top_to_bottom"

    def __str__(self) -> str:
        return str(self.value)
