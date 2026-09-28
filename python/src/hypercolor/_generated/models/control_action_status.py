from enum import StrEnum


class ControlActionStatus(StrEnum):
    ACCEPTED = "accepted"
    COMPLETED = "completed"
    FAILED = "failed"
    RUNNING = "running"

    def __str__(self) -> str:
        return str(self.value)
