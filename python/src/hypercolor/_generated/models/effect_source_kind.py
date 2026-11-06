from ..tolerant_enum import TolerantStrEnum


class EffectSourceKind(TolerantStrEnum):
    HTML = "html"
    NATIVE = "native"
    SHADER = "shader"

    def __str__(self) -> str:
        return str(self.value)
