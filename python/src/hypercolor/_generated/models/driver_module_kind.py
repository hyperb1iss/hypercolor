from enum import Enum


class DriverModuleKind(str, Enum):
    BRIDGE = "bridge"
    HAL = "hal"
    HOST = "host"
    NETWORK = "network"
    VIRTUAL = "virtual"

    def __str__(self) -> str:
        return str(self.value)
