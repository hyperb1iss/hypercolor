from enum import StrEnum


class OutputPowerMode(StrEnum):
    PAUSED = "paused"
    RUNNING = "running"

    def __str__(self) -> str:
        return str(self.value)
