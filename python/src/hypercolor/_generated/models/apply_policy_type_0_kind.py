from enum import StrEnum


class ApplyPolicyType0Kind(StrEnum):
    LIVE = "live"

    def __str__(self) -> str:
        return str(self.value)
