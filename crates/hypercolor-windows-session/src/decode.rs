use hypercolor_types::session::SessionEvent;

/// Win32 `WM_POWERBROADCAST` message identifier.
pub const WM_POWERBROADCAST_MESSAGE: u32 = 0x0218;
/// Win32 `WM_WTSSESSION_CHANGE` message identifier.
pub const WM_WTSSESSION_CHANGE_MESSAGE: u32 = 0x02B1;
/// Win32 suspend notification identifier.
pub const PBT_APMSUSPEND_NOTIFICATION: u32 = 0x0004;
/// Win32 automatic-resume notification identifier.
pub const PBT_APMRESUMEAUTOMATIC_NOTIFICATION: u32 = 0x0012;
/// Win32 user-resume notification identifier.
pub const PBT_APMRESUMESUSPEND_NOTIFICATION: u32 = 0x0007;
/// Win32 critical-resume notification identifier.
pub const PBT_APMRESUMECRITICAL_NOTIFICATION: u32 = 0x0006;
/// Win32 session-lock notification identifier.
pub const WTS_SESSION_LOCK_NOTIFICATION: u32 = 0x0007;
/// Win32 session-unlock notification identifier.
pub const WTS_SESSION_UNLOCK_NOTIFICATION: u32 = 0x0008;

/// Service Control Manager notification translated by the service handler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScmNotification {
    Suspend,
    ResumeAutomatic,
    ResumeInteractive,
    ResumeCritical,
    SessionLocked,
    SessionUnlocked,
    Other,
}

/// Decode a Win32 window message into the neutral session vocabulary.
#[must_use]
pub const fn decode_window_message(message: u32, notification: u32) -> Option<SessionEvent> {
    match (message, notification) {
        (WM_POWERBROADCAST_MESSAGE, PBT_APMSUSPEND_NOTIFICATION) => Some(SessionEvent::Suspending),
        (
            WM_POWERBROADCAST_MESSAGE,
            PBT_APMRESUMEAUTOMATIC_NOTIFICATION
            | PBT_APMRESUMESUSPEND_NOTIFICATION
            | PBT_APMRESUMECRITICAL_NOTIFICATION,
        ) => Some(SessionEvent::Resumed),
        (WM_WTSSESSION_CHANGE_MESSAGE, WTS_SESSION_LOCK_NOTIFICATION) => {
            Some(SessionEvent::ScreenLocked)
        }
        (WM_WTSSESSION_CHANGE_MESSAGE, WTS_SESSION_UNLOCK_NOTIFICATION) => {
            Some(SessionEvent::ScreenUnlocked)
        }
        _ => None,
    }
}

/// Decode an SCM notification into the neutral session vocabulary.
#[must_use]
pub const fn decode_scm_notification(notification: ScmNotification) -> Option<SessionEvent> {
    match notification {
        ScmNotification::Suspend => Some(SessionEvent::Suspending),
        ScmNotification::ResumeAutomatic
        | ScmNotification::ResumeInteractive
        | ScmNotification::ResumeCritical => Some(SessionEvent::Resumed),
        ScmNotification::SessionLocked => Some(SessionEvent::ScreenLocked),
        ScmNotification::SessionUnlocked => Some(SessionEvent::ScreenUnlocked),
        ScmNotification::Other => None,
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn notification_from_service_control(
    control: &windows_service::service::ServiceControl,
) -> ScmNotification {
    use windows_service::service::{PowerEventParam, ServiceControl, SessionChangeReason};

    match control {
        ServiceControl::PowerEvent(PowerEventParam::Suspend) => ScmNotification::Suspend,
        ServiceControl::PowerEvent(PowerEventParam::ResumeAutomatic) => {
            ScmNotification::ResumeAutomatic
        }
        ServiceControl::PowerEvent(PowerEventParam::ResumeSuspend) => {
            ScmNotification::ResumeInteractive
        }
        ServiceControl::PowerEvent(PowerEventParam::ResumeCritical) => {
            ScmNotification::ResumeCritical
        }
        ServiceControl::SessionChange(change)
            if change.reason == SessionChangeReason::SessionLock =>
        {
            ScmNotification::SessionLocked
        }
        ServiceControl::SessionChange(change)
            if change.reason == SessionChangeReason::SessionUnlock =>
        {
            ScmNotification::SessionUnlocked
        }
        _ => ScmNotification::Other,
    }
}
