from ..tolerant_enum import TolerantStrEnum


class OutputPowerMode(TolerantStrEnum):
    PAUSED = "paused"
    RUNNING = "running"

    def __str__(self) -> str:
        return str(self.value)
