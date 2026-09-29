from enum import StrEnum


class BTreeMapAdditionalPropertyType4Kind(StrEnum):
    TEXT = "text"

    def __str__(self) -> str:
        return str(self.value)
