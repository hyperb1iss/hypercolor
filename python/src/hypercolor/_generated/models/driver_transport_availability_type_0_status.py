from enum import StrEnum


class DriverTransportAvailabilityType0Status(StrEnum):
    AVAILABLE = "available"

    def __str__(self) -> str:
        return str(self.value)
