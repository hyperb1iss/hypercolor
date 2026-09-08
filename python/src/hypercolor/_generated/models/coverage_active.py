from enum import Enum


class CoverageActive(str, Enum):
    BRIDGE = "bridge"
    CONFLICT = "conflict"
    NATIVE = "native"
    NONE = "none"

    def __str__(self) -> str:
        return str(self.value)
