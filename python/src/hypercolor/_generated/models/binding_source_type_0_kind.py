from enum import Enum


class BindingSourceType0Kind(str, Enum):
    AUDIO_BAND = "audio_band"

    def __str__(self) -> str:
        return str(self.value)
