from enum import Enum


class BTreeMapAdditionalPropertyType2Kind(str, Enum):
    INT = "int"

    def __str__(self) -> str:
        return str(self.value)
