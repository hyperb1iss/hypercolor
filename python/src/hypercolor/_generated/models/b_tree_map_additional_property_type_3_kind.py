from enum import Enum


class BTreeMapAdditionalPropertyType3Kind(str, Enum):
    FLOAT = "float"

    def __str__(self) -> str:
        return str(self.value)
