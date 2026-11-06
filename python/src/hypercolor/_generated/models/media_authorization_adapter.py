from ..tolerant_enum import TolerantStrEnum


class MediaAuthorizationAdapter(TolerantStrEnum):
    MUSIC = "music"
    SPOTIFY = "spotify"

    def __str__(self) -> str:
        return str(self.value)
