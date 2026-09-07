from enum import Enum


class BTreeMapAdditionalPropertyType5Kind(str, Enum):
    SECRET_REF = "secret_ref"

    def __str__(self) -> str:
        return str(self.value)
