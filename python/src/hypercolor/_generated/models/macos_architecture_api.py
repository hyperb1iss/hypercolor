from enum import Enum


class MacosArchitectureApi(str, Enum):
    APPLE_SILICON = "apple_silicon"
    INTEL = "intel"

    def __str__(self) -> str:
        return str(self.value)
