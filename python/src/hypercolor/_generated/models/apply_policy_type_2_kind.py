from enum import Enum


class ApplyPolicyType2Kind(str, Enum):
    NEXT_SCAN = "next_scan"

    def __str__(self) -> str:
        return str(self.value)
