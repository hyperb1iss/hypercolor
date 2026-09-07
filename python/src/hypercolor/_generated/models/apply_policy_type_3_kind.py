from enum import Enum


class ApplyPolicyType3Kind(str, Enum):
    RESTART = "restart"

    def __str__(self) -> str:
        return str(self.value)
