//! Windows session and power event monitors for Hypercolor.
//!
//! Standalone daemons receive Win32 window messages. Windows services receive
//! equivalent callbacks from the Service Control Manager. Both paths decode
//! into the same neutral [`SessionEvent`] vocabulary before core sees them.

mod decode;
mod scm;

#[cfg(target_os = "windows")]
mod standalone;

use hypercolor_core::session::SessionMonitor;
pub use hypercolor_types::session::SessionEvent;

pub use self::decode::{
    PBT_APMRESUMEAUTOMATIC_NOTIFICATION, PBT_APMRESUMECRITICAL_NOTIFICATION,
    PBT_APMRESUMESUSPEND_NOTIFICATION, PBT_APMSUSPEND_NOTIFICATION, ScmNotification,
    WM_POWERBROADCAST_MESSAGE, WM_WTSSESSION_CHANGE_MESSAGE, WTS_SESSION_LOCK_NOTIFICATION,
    WTS_SESSION_UNLOCK_NOTIFICATION, decode_scm_notification, decode_window_message,
};
pub use self::scm::{ScmSessionEventAdapter, ScmSessionMonitor, scm_session_monitor};

#[cfg(target_os = "windows")]
pub use self::standalone::StandaloneSessionMonitor;

/// Build the standalone Windows session monitor for daemon composition.
#[must_use]
pub fn standalone_monitors() -> Vec<Box<dyn SessionMonitor>> {
    #[cfg(target_os = "windows")]
    {
        vec![Box::new(StandaloneSessionMonitor::new())]
    }

    #[cfg(not(target_os = "windows"))]
    {
        Vec::new()
    }
}
