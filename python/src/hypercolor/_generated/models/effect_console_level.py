from ..tolerant_enum import TolerantStrEnum


class EffectConsoleLevel(TolerantStrEnum):
    DEBUG = "debug"
    ERROR = "error"
    LOG = "log"
    WARN = "warn"

    def __str__(self) -> str:
        return str(self.value)
