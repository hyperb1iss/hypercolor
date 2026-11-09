from ..tolerant_enum import TolerantStrEnum


class PreviewSource(TolerantStrEnum):
    EFFECT_CANVAS = "effect_canvas"
    SCREEN_CAPTURE = "screen_capture"
    WEB_VIEWPORT = "web_viewport"

    def __str__(self) -> str:
        return str(self.value)
