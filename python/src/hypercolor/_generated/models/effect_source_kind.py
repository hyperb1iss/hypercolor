from enum import StrEnum


class EffectSourceKind(StrEnum):
    HTML = "html"
    NATIVE = "native"
    SHADER = "shader"

    def __str__(self) -> str:
        return str(self.value)
