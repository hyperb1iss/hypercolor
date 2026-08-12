from enum import Enum


class MacosSelectionStateApiType2Type(str, Enum):
    SESSION_SCOPED = "session_scoped"

    def __str__(self) -> str:
        return str(self.value)
