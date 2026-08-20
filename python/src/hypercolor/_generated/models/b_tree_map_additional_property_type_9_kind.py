from enum import Enum


class BTreeMapAdditionalPropertyType9Kind(str, Enum):
    MAC_ADDRESS = "mac_address"

    def __str__(self) -> str:
        return str(self.value)
