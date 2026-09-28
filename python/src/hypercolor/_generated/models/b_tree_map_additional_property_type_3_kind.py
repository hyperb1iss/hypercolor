from enum import StrEnum


class BTreeMapAdditionalPropertyType3Kind(StrEnum):
    FLOAT = "float"

    def __str__(self) -> str:
        return str(self.value)
