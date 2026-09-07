from enum import Enum


class PlaylistTargetRequestType1Type(str, Enum):
    PRESET = "preset"

    def __str__(self) -> str:
        return str(self.value)
