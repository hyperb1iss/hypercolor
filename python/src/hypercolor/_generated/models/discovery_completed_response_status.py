from enum import StrEnum


class DiscoveryCompletedResponseStatus(StrEnum):
    COMPLETED = "completed"

    def __str__(self) -> str:
        return str(self.value)
