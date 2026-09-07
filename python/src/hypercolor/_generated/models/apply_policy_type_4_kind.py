from enum import Enum


class ApplyPolicyType4Kind(str, Enum):
    INERT = "inert"

    def __str__(self) -> str:
        return str(self.value)
