from ..tolerant_enum import TolerantStrEnum


class LiveSection(TolerantStrEnum):
    AUDIO = "audio"
    CAPTURE = "capture"
    INPUT = "input"
    RENDER = "render"

    def __str__(self) -> str:
        return str(self.value)
