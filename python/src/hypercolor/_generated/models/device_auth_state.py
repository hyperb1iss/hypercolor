from ..tolerant_enum import TolerantStrEnum


class DeviceAuthState(TolerantStrEnum):
    CONFIGURED = "configured"
    ERROR = "error"
    OPEN = "open"
    REQUIRED = "required"

    def __str__(self) -> str:
        return str(self.value)
