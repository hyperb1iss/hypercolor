from enum import Enum


class ApplyImpactType3(str, Enum):
    DEVICE_RECONNECT = "device_reconnect"

    def __str__(self) -> str:
        return str(self.value)
