from enum import Enum


class ControlGroupKind(str, Enum):
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
