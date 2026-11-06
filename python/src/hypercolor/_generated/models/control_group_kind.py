from ..tolerant_enum import TolerantStrEnum


class ControlGroupKind(TolerantStrEnum):
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
