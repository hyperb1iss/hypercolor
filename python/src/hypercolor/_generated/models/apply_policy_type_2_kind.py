from enum import StrEnum


class ApplyPolicyType2Kind(StrEnum):
    NEXT_SCAN = "next_scan"

    def __str__(self) -> str:
        return str(self.value)
