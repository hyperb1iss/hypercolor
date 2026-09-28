from enum import StrEnum


class LedTopologyType6Type(StrEnum):
    CUSTOM = "custom"

    def __str__(self) -> str:
        return str(self.value)
