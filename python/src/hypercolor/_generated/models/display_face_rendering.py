from ..tolerant_enum import TolerantStrEnum


class DisplayFaceRendering(TolerantStrEnum):
    DEFAULT = "default"
    NONE = "none"
    SCENE = "scene"

    def __str__(self) -> str:
        return str(self.value)
