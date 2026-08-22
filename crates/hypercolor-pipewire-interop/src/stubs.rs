use super::{PortalError, PortalRequest};

/// Stub portal selection on hosts without XDG ScreenCast support.
pub struct PortalSession;

/// Stub portal lifetime guard on hosts without XDG ScreenCast support.
pub struct PortalSessionGuard;

impl PortalSessionGuard {
    /// Reports that the native portal session is unavailable on this platform.
    pub async fn close(self) -> Result<(), PortalError> {
        Err(PortalError::UnsupportedPlatform)
    }
}

/// Reports that XDG ScreenCast capture is unavailable on this platform.
pub async fn open_portal_session(_request: &PortalRequest) -> Result<PortalSession, PortalError> {
    Err(PortalError::UnsupportedPlatform)
}
