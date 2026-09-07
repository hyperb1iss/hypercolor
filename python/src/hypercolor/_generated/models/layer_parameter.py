from enum import Enum


class LayerParameter(str, Enum):
    BRIGHTNESS = "brightness"
    CONTRAST = "contrast"
    HUE_SHIFT = "hue_shift"
    OPACITY = "opacity"
    PLAYBACK_SPEED = "playback_speed"
    ROTATION = "rotation"
    SATURATION = "saturation"
    SCALE_X = "scale_x"
    SCALE_Y = "scale_y"
    TINT_STRENGTH = "tint_strength"

    def __str__(self) -> str:
        return str(self.value)
