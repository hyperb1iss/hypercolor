from ..tolerant_enum import TolerantStrEnum


class DisplayFaceScope(TolerantStrEnum):
    DEFAULT = "default"
    SCENE = "scene"

    def __str__(self) -> str:
        return str(self.value)
