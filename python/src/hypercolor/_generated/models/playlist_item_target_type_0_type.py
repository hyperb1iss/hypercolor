from enum import Enum


class PlaylistItemTargetType0Type(str, Enum):
    EFFECT = "effect"

    def __str__(self) -> str:
        return str(self.value)
