from ..tolerant_enum import TolerantStrEnum


class WebViewportRender(TolerantStrEnum):
    LIVE = "live"
    SNAPSHOT = "snapshot"

    def __str__(self) -> str:
        return str(self.value)
