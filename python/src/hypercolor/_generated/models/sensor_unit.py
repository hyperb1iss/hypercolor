from enum import Enum


class SensorUnit(str, Enum):
    CELSIUS = "celsius"
    MEGABYTES = "megabytes"
    MHZ = "mhz"
    PERCENT = "percent"
    RPM = "rpm"
    WATTS = "watts"

    def __str__(self) -> str:
        return str(self.value)
