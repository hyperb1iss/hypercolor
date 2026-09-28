from enum import StrEnum


class DisplayFaceScope(StrEnum):
    DEFAULT = "default"
    SCENE = "scene"

    def __str__(self) -> str:
        return str(self.value)
