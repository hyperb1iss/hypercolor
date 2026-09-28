from enum import StrEnum


class ApplyPolicyType3Kind(StrEnum):
    RESTART = "restart"

    def __str__(self) -> str:
        return str(self.value)
