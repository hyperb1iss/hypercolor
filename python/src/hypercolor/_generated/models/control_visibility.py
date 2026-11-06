from ..tolerant_enum import TolerantStrEnum


class ControlVisibility(TolerantStrEnum):
    ADVANCED = "advanced"
    DIAGNOSTICS = "diagnostics"
    HIDDEN = "hidden"
    STANDARD = "standard"

    def __str__(self) -> str:
        return str(self.value)
