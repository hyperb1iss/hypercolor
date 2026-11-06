from ..tolerant_enum import TolerantStrEnum


class ApplyImpactType0(TolerantStrEnum):
    BACKEND_REBIND = "backend_rebind"
    DEVICE_RECONNECT = "device_reconnect"
    DISCOVERY_RESCAN = "discovery_rescan"
    HARDWARE_PERSIST = "hardware_persist"
    LIVE = "live"
    NONE = "none"
    TOPOLOGY_REBUILD = "topology_rebuild"

    def __str__(self) -> str:
        return str(self.value)
