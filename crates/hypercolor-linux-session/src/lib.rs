//! Linux session and power event monitors for Hypercolor.

#[cfg(target_os = "linux")]
mod logind;
#[cfg(target_os = "linux")]
mod screensaver;

use hypercolor_core::session::SessionMonitor;
use hypercolor_types::session::SessionConfig;

#[cfg(target_os = "linux")]
pub use self::logind::LogindMonitor;
#[cfg(target_os = "linux")]
pub use self::screensaver::ScreensaverMonitor;

/// Build the Linux session monitors for daemon composition.
#[must_use]
pub fn monitors(config: &SessionConfig) -> Vec<Box<dyn SessionMonitor>> {
    #[cfg(target_os = "linux")]
    {
        vec![
            Box::new(ScreensaverMonitor::new()),
            Box::new(LogindMonitor::new(config)),
        ]
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = config;
        Vec::new()
    }
}
