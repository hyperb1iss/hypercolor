from enum import Enum


class BTreeMapAdditionalPropertyType10Kind(str, Enum):
    DURATION_MS = "duration_ms"

    def __str__(self) -> str:
        return str(self.value)
