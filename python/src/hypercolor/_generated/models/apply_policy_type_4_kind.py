from enum import StrEnum


class ApplyPolicyType4Kind(StrEnum):
    INERT = "inert"

    def __str__(self) -> str:
        return str(self.value)
