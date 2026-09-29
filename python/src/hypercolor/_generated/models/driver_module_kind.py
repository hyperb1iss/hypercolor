from enum import StrEnum


class DriverModuleKind(StrEnum):
    BRIDGE = "bridge"
    HAL = "hal"
    HOST = "host"
    NETWORK = "network"
    VIRTUAL = "virtual"

    def __str__(self) -> str:
        return str(self.value)
