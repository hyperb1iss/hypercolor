from enum import Enum


class MacosProtectedSourceStateApi(str, Enum):
    DISABLED = "disabled"
    FAILED = "failed"
    INTERRUPTED = "interrupted"
    LIVE = "live"
    NEEDS_PROCESS_RESTART = "needs_process_restart"
    NEEDS_SELECTION = "needs_selection"
    NEEDS_USER_ACTION = "needs_user_action"
    PERMISSION_DENIED = "permission_denied"
    READY_IDLE = "ready_idle"
    REVOKED = "revoked"
    STARTING = "starting"

    def __str__(self) -> str:
        return str(self.value)
