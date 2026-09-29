from enum import StrEnum


class LiveSection(StrEnum):
    AUDIO = "audio"
    CAPTURE = "capture"
    INPUT = "input"
    RENDER = "render"

    def __str__(self) -> str:
        return str(self.value)
