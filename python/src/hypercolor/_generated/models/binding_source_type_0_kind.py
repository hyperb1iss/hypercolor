from enum import StrEnum


class BindingSourceType0Kind(StrEnum):
    AUDIO_BAND = "audio_band"

    def __str__(self) -> str:
        return str(self.value)
