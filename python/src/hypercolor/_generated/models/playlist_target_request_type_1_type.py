from enum import StrEnum


class PlaylistTargetRequestType1Type(StrEnum):
    PRESET = "preset"

    def __str__(self) -> str:
        return str(self.value)
