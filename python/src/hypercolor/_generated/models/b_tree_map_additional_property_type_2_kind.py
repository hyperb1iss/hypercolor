from enum import StrEnum


class BTreeMapAdditionalPropertyType2Kind(StrEnum):
    INT = "int"

    def __str__(self) -> str:
        return str(self.value)
