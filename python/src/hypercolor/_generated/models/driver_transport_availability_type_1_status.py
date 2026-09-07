from enum import Enum


class DriverTransportAvailabilityType1Status(str, Enum):
    UNSUPPORTED_PLATFORM = "unsupported_platform"

    def __str__(self) -> str:
        return str(self.value)
