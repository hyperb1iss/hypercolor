from enum import StrEnum


class LoopMode(StrEnum):
    LOOP = "loop"
    NONE = "none"
    PING_PONG = "ping_pong"

    def __str__(self) -> str:
        return str(self.value)
