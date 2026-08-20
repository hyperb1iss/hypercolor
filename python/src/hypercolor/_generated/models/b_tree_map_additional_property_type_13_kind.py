from enum import Enum


class BTreeMapAdditionalPropertyType13Kind(str, Enum):
    LIST = "list"

    def __str__(self) -> str:
        return str(self.value)
