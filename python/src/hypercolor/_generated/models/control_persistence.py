from enum import Enum


class ControlPersistence(str, Enum):
    DEVICE_CONFIG = "device_config"
    DRIVER_CONFIG = "driver_config"
    HARDWARE_STORED = "hardware_stored"
    PROFILE_OVERRIDE = "profile_override"
    RUNTIME_ONLY = "runtime_only"

    def __str__(self) -> str:
        return str(self.value)
