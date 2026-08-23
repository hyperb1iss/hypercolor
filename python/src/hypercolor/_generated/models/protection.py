from enum import Enum


class Protection(str, Enum):
    OPEN = "open"
    SECTION_ROOT = "section_root"
    TREE = "tree"

    def __str__(self) -> str:
        return str(self.value)
