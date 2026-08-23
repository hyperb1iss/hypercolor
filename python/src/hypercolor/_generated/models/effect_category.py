from enum import Enum


class EffectCategory(str, Enum):
    AMBIENT = "ambient"
    AUDIO = "audio"
    DISPLAY = "display"
    FUN = "fun"
    GENERATIVE = "generative"
    INTERACTIVE = "interactive"
    PARTICLE = "particle"
    SCENIC = "scenic"
    SOURCE = "source"
    UTILITY = "utility"

    def __str__(self) -> str:
        return str(self.value)
