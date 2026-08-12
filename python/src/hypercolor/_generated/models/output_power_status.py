from enum import Enum


class OutputPowerStatus(str, Enum):
    PAUSED = "paused"
    RUNNING = "running"
    STOPPED = "stopped"

    def __str__(self) -> str:
        return str(self.value)
