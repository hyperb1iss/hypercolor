from enum import Enum


class ControlVisibility(str, Enum):
    ADVANCED = "advanced"
    DIAGNOSTICS = "diagnostics"
    HIDDEN = "hidden"
    STANDARD = "standard"

    def __str__(self) -> str:
        return str(self.value)
