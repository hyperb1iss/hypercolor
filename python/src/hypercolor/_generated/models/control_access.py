from ..tolerant_enum import TolerantStrEnum


class ControlAccess(TolerantStrEnum):
    READ_ONLY = "read_only"
    READ_WRITE = "read_write"
    WRITE_ONLY = "write_only"

    def __str__(self) -> str:
        return str(self.value)
