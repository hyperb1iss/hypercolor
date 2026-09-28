from enum import StrEnum


class ControlGroupKind(StrEnum):
    ADVANCED = "advanced"
    COLOR = "color"
    CONNECTION = "connection"
    CUSTOM = "custom"
    DANGER = "danger"
    DIAGNOSTICS = "diagnostics"
    GENERAL = "general"
    OUTPUT = "output"
    PERFORMANCE = "performance"
    TOPOLOGY = "topology"

    def __str__(self) -> str:
        return str(self.value)
