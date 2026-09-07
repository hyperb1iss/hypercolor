from enum import Enum


class DisplayPixelFormat(str, Enum):
    RGB = "rgb"
    YUV420 = "yuv420"

    def __str__(self) -> str:
        return str(self.value)
