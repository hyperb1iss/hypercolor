from enum import Enum


class LedTopologyType6Type(str, Enum):
    CUSTOM = "custom"

    def __str__(self) -> str:
        return str(self.value)
