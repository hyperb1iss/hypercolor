from enum import StrEnum


class BTreeMapAdditionalPropertyType1Kind(StrEnum):
    BOOL = "bool"

    def __str__(self) -> str:
        return str(self.value)
