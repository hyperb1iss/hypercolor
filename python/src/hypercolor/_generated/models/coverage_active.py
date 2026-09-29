from enum import StrEnum


class CoverageActive(StrEnum):
    BRIDGE = "bridge"
    CONFLICT = "conflict"
    NATIVE = "native"
    NONE = "none"

    def __str__(self) -> str:
        return str(self.value)
