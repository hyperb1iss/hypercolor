from enum import StrEnum


class DaemonStartupPhase(StrEnum):
    INITIALIZING = "initializing"
    LOADING_STORES = "loading_stores"
    PREPARING_API = "preparing_api"
    PROBING_GPU = "probing_gpu"
    REGISTERING_BACKENDS = "registering_backends"
    SCANNING_EFFECTS = "scanning_effects"
    STARTING_INPUTS = "starting_inputs"
    STARTING_RENDER_THREAD = "starting_render_thread"
    STARTING_SERVICES = "starting_services"
    UNKNOWN = "unknown"

    def __str__(self) -> str:
        return str(self.value)
