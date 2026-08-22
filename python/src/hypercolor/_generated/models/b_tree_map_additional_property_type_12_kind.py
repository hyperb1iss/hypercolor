from enum import Enum


class BTreeMapAdditionalPropertyType12Kind(str, Enum):
    GRADIENT = "gradient"

    def __str__(self) -> str:
        return str(self.value)
