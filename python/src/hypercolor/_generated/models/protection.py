from enum import StrEnum


class Protection(StrEnum):
    OPEN = "open"
    SECTION_ROOT = "section_root"
    TREE = "tree"

    def __str__(self) -> str:
        return str(self.value)
