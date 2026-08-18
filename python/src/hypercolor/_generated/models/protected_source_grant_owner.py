from enum import Enum


class ProtectedSourceGrantOwner(str, Enum):
    APP = "app"
    APP_SIDECAR = "app_sidecar"
    BROKER = "broker"
    HOMEBREW_SERVICE = "homebrew_service"
    LAUNCHD_SERVICE = "launchd_service"
    PLATFORM_BACKEND = "platform_backend"
    STANDALONE = "standalone"

    def __str__(self) -> str:
        return str(self.value)
