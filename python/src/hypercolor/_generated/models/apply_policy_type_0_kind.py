from enum import Enum


class ApplyPolicyType0Kind(str, Enum):
    LIVE = "live"

    def __str__(self) -> str:
        return str(self.value)
