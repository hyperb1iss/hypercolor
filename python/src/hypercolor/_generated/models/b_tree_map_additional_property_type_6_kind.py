from enum import Enum


class BTreeMapAdditionalPropertyType6Kind(str, Enum):
    IP = "ip"

    def __str__(self) -> str:
        return str(self.value)
