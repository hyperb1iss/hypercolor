//! OS signal handling for graceful daemon shutdown.

use tracing::info;

/// Install OS signal handlers for graceful shutdown.
///
/// Returns a watch receiver that flips to `true` when a shutdown signal
/// (Ctrl+C / `SIGTERM`) is received. The spawned task is fire-and-forget;
/// it exits after the first signal.
#[must_use]
pub fn install_signal_handlers() -> tokio::sync::watch::Receiver<bool> {
    install_platform_signal_handlers()
}

#[cfg(unix)]
fn install_platform_signal_handlers() -> tokio::sync::watch::Receiver<bool> {
    let (tx, rx) = tokio::sync::watch::channel(false);

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
