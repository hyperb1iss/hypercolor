from enum import Enum


class BTreeMapAdditionalPropertyType14Kind(str, Enum):
    ENUM = "enum"

    def __str__(self) -> str:
        return str(self.value)
