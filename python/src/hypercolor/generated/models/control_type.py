from enum import Enum


class ControlType(str, Enum):
    COLOR_PICKER = "color_picker"
    DROPDOWN = "dropdown"
    GRADIENT_EDITOR = "gradient_editor"
    RECT = "rect"
    SLIDER = "slider"
    TEXT_INPUT = "text_input"
    TOGGLE = "toggle"

    def __str__(self) -> str:
        return str(self.value)
