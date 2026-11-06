from ..tolerant_enum import TolerantStrEnum


class SensorUnit(TolerantStrEnum):
    CELSIUS = "celsius"
    MEGABYTES = "megabytes"
    MHZ = "mhz"
    PERCENT = "percent"
    RPM = "rpm"
    WATTS = "watts"

    def __str__(self) -> str:
        return str(self.value)
