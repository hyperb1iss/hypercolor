from enum import Enum


class PreviewSource(str, Enum):
    EFFECT_CANVAS = "effect_canvas"
    SCREEN_CAPTURE = "screen_capture"
    WEB_VIEWPORT = "web_viewport"

    def __str__(self) -> str:
        return str(self.value)
