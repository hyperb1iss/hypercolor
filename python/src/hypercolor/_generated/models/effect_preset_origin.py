from ..tolerant_enum import TolerantStrEnum


class EffectPresetOrigin(TolerantStrEnum):
    BUNDLED = "bundled"
    SAVED = "saved"

    def __str__(self) -> str:
        return str(self.value)
