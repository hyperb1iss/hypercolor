from ..tolerant_enum import TolerantStrEnum


class DisplayPixelFormat(TolerantStrEnum):
    RGB = "rgb"
    YUV420 = "yuv420"

    def __str__(self) -> str:
        return str(self.value)
