from enum import StrEnum


class ApplyImpactType3(StrEnum):
    DEVICE_RECONNECT = "device_reconnect"

    def __str__(self) -> str:
        return str(self.value)
