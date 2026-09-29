from enum import StrEnum


class ApplyImpactType5(StrEnum):
    TOPOLOGY_REBUILD = "topology_rebuild"

    def __str__(self) -> str:
        return str(self.value)
