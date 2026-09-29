from enum import StrEnum


class ApplyPolicyType1Kind(StrEnum):
    LIVE_ON_READ = "live_on_read"

    def __str__(self) -> str:
        return str(self.value)
