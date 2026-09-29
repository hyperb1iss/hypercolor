from enum import StrEnum


class BTreeMapAdditionalPropertyType5Kind(StrEnum):
    SECRET_REF = "secret_ref"

    def __str__(self) -> str:
        return str(self.value)
