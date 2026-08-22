from enum import Enum


class AssetWarningType0Kind(str, Enum):
    PER_ASSET_SOFT_CAP_EXCEEDED = "per_asset_soft_cap_exceeded"

    def __str__(self) -> str:
        return str(self.value)
