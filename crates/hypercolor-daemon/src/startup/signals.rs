//! OS signal handling for graceful daemon shutdown.

use tracing::info;

/// Environment variable a supervising app sets to its own pid when it
/// spawns the daemon as a managed child. Arms the parent-death watch so a
/// supervisor that dies without reaping (crash, SIGKILL, exit paths that
/// skip destructors) cannot leave an orphaned daemon holding the port and
/// the ownership guard.
pub const SUPERVISED_PARENT_PID_ENV: &str = "HYPERCOLOR_SUPERVISED_PARENT_PID";

/// Install OS signal handlers for graceful shutdown.
///
/// Returns a watch receiver that flips to `true` when a shutdown signal
/// (Ctrl+C / `SIGTERM`) is received, or when the supervising parent
/// process named by [`SUPERVISED_PARENT_PID_ENV`] dies. The spawned tasks
/// are fire-and-forget; each exits after its first trigger.
#[must_use]
pub fn install_signal_handlers() -> tokio::sync::watch::Receiver<bool> {
    install_platform_signal_handlers()
}

#[cfg(unix)]
fn install_platform_signal_handlers() -> tokio::sync::watch::Receiver<bool> {
    let (tx, rx) = tokio::sync::watch::channel(false);
    let tx = std::sync::Arc::new(tx);

    install_supervised_parent_watch(std::sync::Arc::clone(&tx));

    tokio::spawn(async move {
        let mut terminate =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(terminate) => Some(terminate),
                Err(error) => {
                    tracing::error!(%error, "Failed to listen for SIGTERM shutdown signal");
                    None
                }
            };

        let reason = if let Some(terminate) = terminate.as_mut() {
            tokio::select! {
                result = tokio::signal::ctrl_c() => {
                    if let Err(error) = result {
                        tracing::error!(%error, "Failed to listen for Ctrl+C shutdown signal");
                        return;
                    }
                    "Ctrl+C"
                }
                _ = terminate.recv() => "SIGTERM",
            }
        } else {
            if let Err(error) = tokio::signal::ctrl_c().await {
                tracing::error!(%error, "Failed to listen for Ctrl+C shutdown signal");
                return;
            }
            "Ctrl+C"
        };

        info!(signal = reason, "Shutdown signal received");
        let _ = tx.send(true);
    });

    rx
}

/// Watch the supervising parent process and shut down when it dies.
///
/// A dead parent reparents this process to launchd, so a change in
/// `getppid` is the death signal. The watch arms only when the claimed
/// supervisor pid matches the live parent: a mismatch means the claim was
/// inherited through an exec chain and watching would track the wrong
/// process.
#[cfg(unix)]
fn install_supervised_parent_watch(tx: std::sync::Arc<tokio::sync::watch::Sender<bool>>) {
    let Some(claimed) = std::env::var(SUPERVISED_PARENT_PID_ENV)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
    else {
        return;
    };
    let initial = std::os::unix::process::parent_id();
    if initial != claimed {
        tracing::warn!(
            claimed,
            observed = initial,
            "supervised parent claim does not match the live parent; parent-death watch disarmed"
        );
        return;
    }
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            if std::os::unix::process::parent_id() != initial {
                info!(
                    supervisor_pid = initial,
                    "supervising app exited; daemon shutting down"
                );
                let _ = tx.send(true);
                return;
            }
        }
    });
}

#[cfg(not(unix))]
fn install_platform_signal_handlers() -> tokio::sync::watch::Receiver<bool> {
    let (tx, rx) = tokio::sync::watch::channel(false);

    tokio::spawn(async move {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "Failed to listen for Ctrl+C shutdown signal");
            return;
        }
        info!("Shutdown signal received (Ctrl+C)");
        let _ = tx.send(true);
    });

    rx
}
