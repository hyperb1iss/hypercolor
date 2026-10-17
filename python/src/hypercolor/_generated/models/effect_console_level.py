from enum import StrEnum


class EffectConsoleLevel(StrEnum):
    DEBUG = "debug"
    ERROR = "error"
    LOG = "log"
    WARN = "warn"

    def __str__(self) -> str:
        return str(self.value)
