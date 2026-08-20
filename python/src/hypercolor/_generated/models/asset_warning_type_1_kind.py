from enum import Enum


class AssetWarningType1Kind(str, Enum):
    LIBRARY_SOFT_CAP_EXCEEDED = "library_soft_cap_exceeded"

    def __str__(self) -> str:
        return str(self.value)
