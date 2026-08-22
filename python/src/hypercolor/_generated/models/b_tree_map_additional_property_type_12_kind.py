from enum import Enum


class BTreeMapAdditionalPropertyType12Kind(str, Enum):
    FLAGS = "flags"

    def __str__(self) -> str:
        return str(self.value)
