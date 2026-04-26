from enum import Enum


class DeviceAuthState(str, Enum):
    CONFIGURED = "configured"
    ERROR = "error"
    OPEN = "open"
    REQUIRED = "required"

    def __str__(self) -> str:
        return str(self.value)
