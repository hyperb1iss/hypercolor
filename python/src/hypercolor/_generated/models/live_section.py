from enum import Enum


class LiveSection(str, Enum):
    AUDIO = "audio"
    CAPTURE = "capture"
    INPUT = "input"
    RENDER = "render"

    def __str__(self) -> str:
        return str(self.value)
