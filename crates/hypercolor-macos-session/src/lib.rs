//! macOS session and system-power monitoring for Hypercolor.
//!
//! AppKit and IOKit notifications are normalized into the shared
//! [`SessionEvent`] vocabulary before they cross into core.

mod decode;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_os = "macos"))]
mod stubs;

use hypercolor_core::session::SessionMonitor;
pub use hypercolor_types::session::SessionEvent;

pub use self::decode::{MacosSessionNotification, decode_session_notification};
#[cfg(target_os = "macos")]
pub use self::macos::MacosSessionMonitor;

/// Build the macOS session monitor for daemon composition.
#[must_use]
pub fn monitors() -> Vec<Box<dyn SessionMonitor>> {
    #[cfg(target_os = "macos")]
    {
        vec![Box::new(MacosSessionMonitor::new())]
    }

    #[cfg(not(target_os = "macos"))]
    {
        self::stubs::monitors()
    }
}
