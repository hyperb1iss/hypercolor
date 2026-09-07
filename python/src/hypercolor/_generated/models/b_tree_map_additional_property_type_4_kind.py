from enum import Enum


class BTreeMapAdditionalPropertyType4Kind(str, Enum):
    TEXT = "text"

    def __str__(self) -> str:
        return str(self.value)
