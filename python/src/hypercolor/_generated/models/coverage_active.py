from ..tolerant_enum import TolerantStrEnum


class CoverageActive(TolerantStrEnum):
    BRIDGE = "bridge"
    CONFLICT = "conflict"
    NATIVE = "native"
    NONE = "none"

    def __str__(self) -> str:
        return str(self.value)
