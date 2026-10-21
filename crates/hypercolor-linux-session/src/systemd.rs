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
        if let Err(error) = sd_notify::notify(&[sd_notify::NotifyState::Ready]) {
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
    spawn_watchdog_while(|| true);
}

/// Spawn the periodic watchdog keepalive, sending each keepalive only while
/// `healthy` returns `true`.
///
/// systemd kills and restarts a service that goes `WatchdogSec` without a
/// keepalive, so withholding them is how a daemon with a wedged subsystem
/// it cannot recover in-process gets a fresh process. `healthy` runs once
/// per keepalive interval on the runtime, so it must not block.
///
/// Off Linux this does nothing. On Linux it must run inside a Tokio runtime.
pub fn spawn_watchdog_while<F>(healthy: F)
where
    F: Fn() -> bool + Send + 'static,
{
    #[cfg(target_os = "linux")]
    {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(WATCHDOG_INTERVAL);
            let mut withheld = false;
            loop {
                interval.tick().await;
                if !healthy() {
                    if !withheld {
                        tracing::error!(
                            "withholding the systemd watchdog keepalive because the daemon \
                             reports itself unhealthy; systemd restarts it once the watchdog \
                             timeout passes"
                        );
                        withheld = true;
                    }
                    continue;
                }
                if withheld {
                    tracing::warn!("daemon is healthy again; resuming systemd watchdog keepalives");
                    withheld = false;
                }
                if let Err(error) = sd_notify::notify(&[sd_notify::NotifyState::Watchdog]) {
                    tracing::debug!("failed to notify systemd watchdog: {error}");
                }
            }
        });
    }
    #[cfg(not(target_os = "linux"))]
    let _ = healthy;
}
