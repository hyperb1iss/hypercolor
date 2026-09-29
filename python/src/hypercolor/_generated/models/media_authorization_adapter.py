from enum import StrEnum


class MediaAuthorizationAdapter(StrEnum):
    MUSIC = "music"
    SPOTIFY = "spotify"

    def __str__(self) -> str:
        return str(self.value)
