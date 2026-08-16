from enum import Enum


class ApplyPolicyType1Kind(str, Enum):
    LIVE_ON_READ = "live_on_read"

    def __str__(self) -> str:
        return str(self.value)
