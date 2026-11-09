from ..tolerant_enum import TolerantStrEnum


class ControlActionStatus(TolerantStrEnum):
    ACCEPTED = "accepted"
    COMPLETED = "completed"
    FAILED = "failed"
    RUNNING = "running"

    def __str__(self) -> str:
        return str(self.value)
