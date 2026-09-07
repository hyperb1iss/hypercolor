from enum import Enum


class MediaAuthorizationAdapter(str, Enum):
    MUSIC = "music"
    SPOTIFY = "spotify"

    def __str__(self) -> str:
        return str(self.value)
