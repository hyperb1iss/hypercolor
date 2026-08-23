from enum import Enum


class BTreeMapAdditionalPropertyType0Kind(str, Enum):
    NULL = "null"

    def __str__(self) -> str:
        return str(self.value)
