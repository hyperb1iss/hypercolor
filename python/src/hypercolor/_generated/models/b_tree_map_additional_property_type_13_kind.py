from enum import Enum


class BTreeMapAdditionalPropertyType13Kind(str, Enum):
    RECT = "rect"

    def __str__(self) -> str:
        return str(self.value)
