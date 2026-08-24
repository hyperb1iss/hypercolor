//! systemd readiness and watchdog notifications.
//!
//! Every function is a no-op off Linux so the daemon composes against one
//! signature on every platform. On Linux the calls talk to `NOTIFY_SOCKET`
//! through `sd-notify`, which is itself inert when systemd did not launch
//! the process.

/// Interval between systemd watchdog keepalives.
#[cfg(target_os = "linux")]
const WATCHDOG_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

/// Tell the service manager the daemon is ready to serve.
pub fn notify_ready() {
    #[cfg(target_os = "linux")]
    {
        if let Err(error) = sd_notify::notify(false, &[sd_notify::NotifyState::Ready]) {
            tracing::warn!("failed to notify systemd: {error}");
        } else {
            tracing::debug!("notified systemd: READY=1");
        }
    }
}

/// Spawn the periodic watchdog keepalive on the current Tokio runtime.
///
/// Off Linux this does nothing. On Linux it must run inside a Tokio runtime.
pub fn spawn_watchdog() {
    #[cfg(target_os = "linux")]
    {
        tokio::spawn(async {
            let mut interval = tokio::time::interval(WATCHDOG_INTERVAL);
            loop {
                interval.tick().await;
                if let Err(error) = sd_notify::notify(false, &[sd_notify::NotifyState::Watchdog]) {
                    tracing::debug!("failed to notify systemd watchdog: {error}");
                }
            }
        });
    }
}
