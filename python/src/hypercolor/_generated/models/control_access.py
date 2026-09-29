from enum import StrEnum


class ControlAccess(StrEnum):
    READ_ONLY = "read_only"
    READ_WRITE = "read_write"
    WRITE_ONLY = "write_only"

    def __str__(self) -> str:
        return str(self.value)
