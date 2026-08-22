from enum import Enum


class LoopMode(str, Enum):
    LOOP = "loop"
    NONE = "none"
    PING_PONG = "ping_pong"

    def __str__(self) -> str:
        return str(self.value)
