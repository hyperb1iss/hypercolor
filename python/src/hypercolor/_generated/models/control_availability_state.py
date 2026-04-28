from enum import Enum


class ControlAvailabilityState(str, Enum):
    AVAILABLE = "available"
    DISABLED = "disabled"
    HIDDEN = "hidden"
    READ_ONLY = "read_only"
    UNSUPPORTED = "unsupported"

    def __str__(self) -> str:
        return str(self.value)
