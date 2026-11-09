from ..tolerant_enum import TolerantStrEnum


class DriverModuleKind(TolerantStrEnum):
    BRIDGE = "bridge"
    HAL = "hal"
    HOST = "host"
    NETWORK = "network"
    VIRTUAL = "virtual"

    def __str__(self) -> str:
        return str(self.value)
