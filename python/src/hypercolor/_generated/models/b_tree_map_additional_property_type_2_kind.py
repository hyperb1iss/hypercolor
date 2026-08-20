from enum import Enum


class BTreeMapAdditionalPropertyType2Kind(str, Enum):
    INTEGER = "integer"

    def __str__(self) -> str:
        return str(self.value)
