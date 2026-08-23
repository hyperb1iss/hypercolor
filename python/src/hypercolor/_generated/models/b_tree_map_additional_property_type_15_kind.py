from enum import Enum


class BTreeMapAdditionalPropertyType15Kind(str, Enum):
    FLAGS = "flags"

    def __str__(self) -> str:
        return str(self.value)
