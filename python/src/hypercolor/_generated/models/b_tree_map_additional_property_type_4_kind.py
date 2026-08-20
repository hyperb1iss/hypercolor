from enum import Enum


class BTreeMapAdditionalPropertyType4Kind(str, Enum):
    STRING = "string"

    def __str__(self) -> str:
        return str(self.value)
