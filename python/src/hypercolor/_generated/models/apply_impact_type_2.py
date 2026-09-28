from enum import StrEnum


class ApplyImpactType2(StrEnum):
    DISCOVERY_RESCAN = "discovery_rescan"

    def __str__(self) -> str:
        return str(self.value)
