from enum import Enum


class BTreeMapAdditionalPropertyType8Kind(str, Enum):
    DURATION = "duration"

    def __str__(self) -> str:
        return str(self.value)
