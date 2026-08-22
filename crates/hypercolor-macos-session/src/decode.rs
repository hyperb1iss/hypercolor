use hypercolor_types::session::SessionEvent;

/// Native macOS session notification normalized by this adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacosSessionNotification {
    /// The active workspace session resigned.
    SessionResigned,
    /// The workspace session became active again.
    SessionBecameActive,
    /// IOKit announced non-abortable system sleep.
    SystemWillSleep,
    /// IOKit announced completed system wake.
    SystemPoweredOn,
    /// A notification outside the session contract.
    Other,
}

/// Decode a native macOS notification into the neutral session vocabulary.
#[must_use]
pub const fn decode_session_notification(
    notification: MacosSessionNotification,
) -> Option<SessionEvent> {
    match notification {
        MacosSessionNotification::SessionResigned => Some(SessionEvent::SessionInactive),
        MacosSessionNotification::SessionBecameActive => Some(SessionEvent::SessionActive),
        MacosSessionNotification::SystemWillSleep => Some(SessionEvent::Suspending),
        MacosSessionNotification::SystemPoweredOn => Some(SessionEvent::Resumed),
        MacosSessionNotification::Other => None,
    }
}
