from enum import StrEnum


class PreviewSource(StrEnum):
    EFFECT_CANVAS = "effect_canvas"
    SCREEN_CAPTURE = "screen_capture"
    WEB_VIEWPORT = "web_viewport"

    def __str__(self) -> str:
        return str(self.value)
