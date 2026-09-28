from enum import StrEnum


class ControlPersistence(StrEnum):
    DEVICE_CONFIG = "device_config"
    DRIVER_CONFIG = "driver_config"
    HARDWARE_STORED = "hardware_stored"
    RUNTIME_ONLY = "runtime_only"

    def __str__(self) -> str:
        return str(self.value)
