from enum import Enum


class BTreeMapAdditionalPropertyType1Kind(str, Enum):
    BOOL = "bool"

    def __str__(self) -> str:
        return str(self.value)
