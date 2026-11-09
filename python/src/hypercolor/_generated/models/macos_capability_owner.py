from ..tolerant_enum import TolerantStrEnum


class MacosCapabilityOwner(TolerantStrEnum):
    APP = "app"
    APP_SIDECAR = "app_sidecar"
    BROKER = "broker"
    HOMEBREW_SERVICE = "homebrew_service"
    LAUNCHD_SERVICE = "launchd_service"
    STANDALONE = "standalone"

    def __str__(self) -> str:
        return str(self.value)
