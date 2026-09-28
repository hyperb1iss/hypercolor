from enum import StrEnum


class BTreeMapAdditionalPropertyType8Kind(StrEnum):
    DURATION = "duration"

    def __str__(self) -> str:
        return str(self.value)
