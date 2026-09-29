from enum import StrEnum


class EffectPresetOrigin(StrEnum):
    BUNDLED = "bundled"
    SAVED = "saved"

    def __str__(self) -> str:
        return str(self.value)
