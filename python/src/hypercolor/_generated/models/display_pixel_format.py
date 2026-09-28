from enum import StrEnum


class DisplayPixelFormat(StrEnum):
    RGB = "rgb"
    YUV420 = "yuv420"

    def __str__(self) -> str:
        return str(self.value)
