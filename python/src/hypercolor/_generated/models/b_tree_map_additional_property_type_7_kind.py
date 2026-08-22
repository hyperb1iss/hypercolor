from enum import Enum


class BTreeMapAdditionalPropertyType7Kind(str, Enum):
    MAC = "mac"

    def __str__(self) -> str:
        return str(self.value)
