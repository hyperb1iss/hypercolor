from enum import Enum


class DisplayFaceScope(str, Enum):
    DEFAULT = "default"
    SCENE = "scene"

    def __str__(self) -> str:
        return str(self.value)
