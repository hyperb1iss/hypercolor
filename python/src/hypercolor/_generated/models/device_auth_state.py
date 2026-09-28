from enum import StrEnum


class DeviceAuthState(StrEnum):
    CONFIGURED = "configured"
    ERROR = "error"
    OPEN = "open"
    REQUIRED = "required"

    def __str__(self) -> str:
        return str(self.value)
