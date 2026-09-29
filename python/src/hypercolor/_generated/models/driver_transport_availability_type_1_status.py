from enum import StrEnum


class DriverTransportAvailabilityType1Status(StrEnum):
    UNSUPPORTED_PLATFORM = "unsupported_platform"

    def __str__(self) -> str:
        return str(self.value)
