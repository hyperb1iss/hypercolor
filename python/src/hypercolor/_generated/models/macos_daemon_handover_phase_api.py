from enum import Enum


class MacosDaemonHandoverPhaseApi(str, Enum):
    AUTOSTARTS_CONFIGURED = "autostarts_configured"
    AWAITING_GUARD_RELEASE = "awaiting_guard_release"
    COMMITTED = "committed"
    COMMIT_PENDING = "commit_pending"
    GUARD_RELEASED = "guard_released"
    OUTGOING_OWNER_STOPPED = "outgoing_owner_stopped"
    PREPARED = "prepared"
    PRIOR_OWNER_STARTED = "prior_owner_started"
    REQUESTED_OWNER_STARTED = "requested_owner_started"
    ROLLBACK_AUTOSTARTS_RESTORED = "rollback_autostarts_restored"
    ROLLBACK_AWAITING_GUARD_RELEASE = "rollback_awaiting_guard_release"
    ROLLBACK_COMMIT_PENDING = "rollback_commit_pending"
    ROLLBACK_GUARD_RELEASED = "rollback_guard_released"
    ROLLBACK_OWNER_STOPPED = "rollback_owner_stopped"
    ROLLBACK_PENDING = "rollback_pending"
    ROLLBACK_START_REQUESTED = "rollback_start_requested"
    ROLLBACK_STOP_REQUESTED = "rollback_stop_requested"
    ROLLED_BACK = "rolled_back"
    START_REQUESTED = "start_requested"
    STOP_REQUESTED = "stop_requested"

    def __str__(self) -> str:
        return str(self.value)
