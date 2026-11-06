from ..tolerant_enum import TolerantStrEnum


class Protection(TolerantStrEnum):
    OPEN = "open"
    SECTION_ROOT = "section_root"
    TREE = "tree"

    def __str__(self) -> str:
        return str(self.value)
