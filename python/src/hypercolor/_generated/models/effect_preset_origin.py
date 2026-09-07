from enum import Enum


class EffectPresetOrigin(str, Enum):
    BUNDLED = "bundled"
    SAVED = "saved"

    def __str__(self) -> str:
        return str(self.value)
