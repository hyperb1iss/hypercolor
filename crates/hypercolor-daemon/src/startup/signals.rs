//! OS signal handling for graceful daemon shutdown.

use tracing::info;

/// Environment variable a supervising app sets to its own pid when it
/// spawns the daemon as a managed child. Arms the parent-death watch so a
/// supervisor that dies without reaping (crash, SIGKILL, exit paths that
/// skip destructors) cannot leave an orphaned daemon holding the port and
/// the ownership guard.
pub use hypercolor_types::service::SUPERVISED_PARENT_PID_ENV;

/// How the platform tells a supervised daemon that its supervisor died.
pub enum ParentLifetime {
    /// The kernel delivers the death itself: a Linux pdeathsig SIGTERM or a
    /// Windows job object, so no daemon-side watch is needed.
    Kernel,
    /// The composition root supplies a blocking waiter that returns when the
    /// parent with the given pid exits (the macOS kqueue `EVFILT_PROC` watch).
    Watch(Box<dyn FnOnce(u32) + Send + 'static>),
}

/// Install OS signal handlers for graceful shutdown.
///
/// Returns a watch receiver that flips to `true` when a shutdown signal
/// (Ctrl+C / `SIGTERM`) is received, or when the supervising parent
/// process named by [`SUPERVISED_PARENT_PID_ENV`] dies. The spawned tasks
/// are fire-and-forget; each exits after its first trigger.
#[must_use]
pub fn install_signal_handlers(
    parent_lifetime: ParentLifetime,
) -> tokio::sync::watch::Receiver<bool> {
    let claimed_parent = std::env::var(SUPERVISED_PARENT_PID_ENV)
        .ok()
        .and_then(|value| value.parse::<u32>().ok());
    install_signal_handlers_with_parent_claim(claimed_parent, parent_lifetime)
}

/// [`install_signal_handlers`] with the supervised-parent claim supplied
/// directly instead of read from the environment.
#[must_use]
pub fn install_signal_handlers_with_parent_claim(
    claimed_parent: Option<u32>,
    parent_lifetime: ParentLifetime,
) -> tokio::sync::watch::Receiver<bool> {
    install_platform_signal_handlers(claimed_parent, parent_lifetime)
}

#[cfg(unix)]
fn install_platform_signal_handlers(
    claimed_parent: Option<u32>,
    parent_lifetime: ParentLifetime,
) -> tokio::sync::watch::Receiver<bool> {
    let (tx, rx) = tokio::sync::watch::channel(false);
    let tx = std::sync::Arc::new(tx);

    install_supervised_parent_watch(std::sync::Arc::clone(&tx), claimed_parent, parent_lifetime);

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
/// The watch arms only when the claimed supervisor pid matches the live
/// parent: a mismatch means the claim was inherited through an exec chain
/// and watching would track the wrong process. With a kernel lifetime the
/// supervisor's death already arrives as SIGTERM, so nothing is spawned;
/// with a platform waiter, a blocking thread waits on the parent pid and
/// flips the shutdown watch when it exits.
#[cfg(unix)]
fn install_supervised_parent_watch(
    tx: std::sync::Arc<tokio::sync::watch::Sender<bool>>,
    claimed_parent: Option<u32>,
    parent_lifetime: ParentLifetime,
) {
    let Some(claimed) = claimed_parent else {
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
    match parent_lifetime {
        ParentLifetime::Kernel => {
            info!(
                supervisor_pid = initial,
                "supervised by the kernel parent-death signal"
            );
        }
        ParentLifetime::Watch(wait_for_exit) => {
            tokio::task::spawn_blocking(move || {
                wait_for_exit(initial);
                info!(
                    supervisor_pid = initial,
                    "supervising app exited; daemon shutting down"
                );
                let _ = tx.send(true);
            });
        }
    }
}

#[cfg(not(unix))]
fn install_platform_signal_handlers(
    claimed_parent: Option<u32>,
    parent_lifetime: ParentLifetime,
) -> tokio::sync::watch::Receiver<bool> {
    // Windows binds the child to the supervisor's job object; the kernel
    // terminates it when the job closes.
    let _ = (claimed_parent, parent_lifetime);
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
