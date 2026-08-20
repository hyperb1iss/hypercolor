from enum import Enum


class LayerBlendMode(str, Enum):
    ADD = "add"
    ALPHA = "alpha"
    COLOR_DODGE = "color_dodge"
    DIFFERENCE = "difference"
    LUMA_REVEAL = "luma_reveal"
    MULTIPLY = "multiply"
    OVERLAY = "overlay"
    REPLACE = "replace"
    SCREEN = "screen"
    SOFT_LIGHT = "soft_light"
    TINT = "tint"

    def __str__(self) -> str:
        return str(self.value)
