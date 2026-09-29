from enum import StrEnum


class BTreeMapAdditionalPropertyType16Kind(StrEnum):
    LIST = "list"

    def __str__(self) -> str:
        return str(self.value)
