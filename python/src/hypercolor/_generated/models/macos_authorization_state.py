from enum import Enum


class MacosAuthorizationState(str, Enum):
    AUTHORIZED = "authorized"
    DENIED = "denied"
    NOT_DETERMINED = "not_determined"
    UNKNOWN = "unknown"

    def __str__(self) -> str:
        return str(self.value)
