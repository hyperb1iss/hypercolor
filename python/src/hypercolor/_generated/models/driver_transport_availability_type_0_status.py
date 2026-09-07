from enum import Enum


class DriverTransportAvailabilityType0Status(str, Enum):
    AVAILABLE = "available"

    def __str__(self) -> str:
        return str(self.value)
