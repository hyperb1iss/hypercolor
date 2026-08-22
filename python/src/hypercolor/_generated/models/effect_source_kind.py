from enum import Enum


class EffectSourceKind(str, Enum):
    HTML = "html"
    NATIVE = "native"
    SHADER = "shader"

    def __str__(self) -> str:
        return str(self.value)
