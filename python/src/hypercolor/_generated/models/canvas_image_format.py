from ..tolerant_enum import TolerantStrEnum


class CanvasImageFormat(TolerantStrEnum):
    JPEG = "jpeg"
    PNG = "png"

    def __str__(self) -> str:
        return str(self.value)
