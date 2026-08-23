from enum import Enum


class MacosArchitecture(str, Enum):
    APPLE_SILICON = "apple_silicon"
    INTEL = "intel"

    def __str__(self) -> str:
        return str(self.value)
