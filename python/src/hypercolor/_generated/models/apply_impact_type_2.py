from enum import Enum


class ApplyImpactType2(str, Enum):
    DISCOVERY_RESCAN = "discovery_rescan"

    def __str__(self) -> str:
        return str(self.value)
