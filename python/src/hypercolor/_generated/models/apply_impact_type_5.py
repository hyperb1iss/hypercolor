from enum import Enum


class ApplyImpactType5(str, Enum):
    TOPOLOGY_REBUILD = "topology_rebuild"

    def __str__(self) -> str:
        return str(self.value)
