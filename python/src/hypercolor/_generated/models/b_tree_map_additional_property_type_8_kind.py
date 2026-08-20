from enum import Enum


class BTreeMapAdditionalPropertyType8Kind(str, Enum):
    IP_ADDRESS = "ip_address"

    def __str__(self) -> str:
        return str(self.value)
