from enum import StrEnum


class ControlVisibility(StrEnum):
    ADVANCED = "advanced"
    DIAGNOSTICS = "diagnostics"
    HIDDEN = "hidden"
    STANDARD = "standard"

    def __str__(self) -> str:
        return str(self.value)
