from ..tolerant_enum import TolerantStrEnum


class LoopMode(TolerantStrEnum):
    LOOP = "loop"
    NONE = "none"
    PING_PONG = "ping_pong"

    def __str__(self) -> str:
        return str(self.value)
