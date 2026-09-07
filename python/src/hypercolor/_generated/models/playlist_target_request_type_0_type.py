from enum import Enum


class PlaylistTargetRequestType0Type(str, Enum):
    EFFECT = "effect"

    def __str__(self) -> str:
        return str(self.value)
