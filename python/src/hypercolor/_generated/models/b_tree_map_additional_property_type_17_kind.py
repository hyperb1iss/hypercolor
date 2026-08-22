from enum import Enum


class BTreeMapAdditionalPropertyType17Kind(str, Enum):
    MAP = "map"

    def __str__(self) -> str:
        return str(self.value)
