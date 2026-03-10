//! Linux session monitor backed by systemd-logind.

use std::env;
use std::time::Duration;

use anyhow::Context;
use futures_core::Stream;
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use zbus::Connection;
use zbus::zvariant::{OwnedFd, OwnedObjectPath};

use crate::session::SessionMonitor;
use crate::types::session::{SessionConfig, SessionEvent};

const INHIBITOR_GRACE: Duration = Duration::from_millis(100);
const RECONNECT_BACKOFF: Duration = Duration::from_secs(1);

#[zbus::proxy(
    interface = "org.freedesktop.login1.Manager",
    default_service = "org.freedesktop.login1",
    default_path = "/org/freedesktop/login1"
)]
trait LoginManager {
    fn get_session(&self, session_id: &str) -> zbus::Result<OwnedObjectPath>;

    fn inhibit(&self, what: &str, who: &str, why: &str, mode: &str) -> zbus::Result<OwnedFd>;

    #[zbus(signal)]
    fn prepare_for_sleep(&self, start: bool) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.login1.Session",
    default_service = "org.freedesktop.login1"
)]
trait LoginSession {
    #[zbus(signal)]
    fn lock(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn unlock(&self) -> zbus::Result<()>;

    #[zbus(property)]
    fn locked_hint(&self) -> zbus::Result<bool>;
}

/// logind-backed monitor for suspend/resume and lock/unlock events.
pub struct LogindMonitor {
    suspend_fade: Duration,
}

impl LogindMonitor {
    /// Create a new monitor from the current session config.
    #[must_use]
    pub fn new(config: &SessionConfig) -> Self {
        Self {
            suspend_fade: Duration::from_millis(config.suspend_fade_ms),
        }
    }
}

#[async_trait::async_trait]
impl SessionMonitor for LogindMonitor {
    fn name(&self) -> &'static str {
        "logind"
    }

    async fn run(
        self,
        tx: mpsc::Sender<SessionEvent>,
        cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        let mut connected_once = false;

        loop {
            match run_logind_monitor_once(self.suspend_fade, &tx, &cancel).await {
                Ok(RunResult::Cancelled) => break,
                Ok(RunResult::Reconnect) => {
                    connected_once = true;
                    warn!("logind monitor connection dropped; rebuilding subscriptions");
                    tokio::select! {
                        () = cancel.cancelled() => break,
                        () = tokio::time::sleep(RECONNECT_BACKOFF) => {}
                    }
                }
                Err(error) => {
                    if !connected_once {
                        return Err(error);
                    }

                    warn!(%error, "logind monitor reconnect failed; retrying");
                    tokio::select! {
                        () = cancel.cancelled() => break,
                        () = tokio::time::sleep(RECONNECT_BACKOFF) => {}
                    }
                }
            }
        }

        Ok(())
    }
}

enum RunResult {
    Cancelled,
    Reconnect,
}

#[allow(
    clippy::too_many_lines,
    reason = "logind session monitor is a single event loop with sequential D-Bus setup"
)]
async fn run_logind_monitor_once(
    suspend_fade: Duration,
    tx: &mpsc::Sender<SessionEvent>,
    cancel: &CancellationToken,
) -> anyhow::Result<RunResult> {
    let connection = Connection::system()
        .await
        .context("failed to connect to the system D-Bus")?;
    let manager = LoginManagerProxy::new(&connection)
        .await
        .context("failed to build login1 manager proxy")?;

    let session_proxy = match resolve_session_proxy(&connection, &manager).await {
        Ok(proxy) => Some(proxy),
        Err(error) => {
            warn!(%error, "logind session lock monitoring unavailable");
            None
        }
    };

    if let Some(proxy) = &session_proxy {
        match proxy.locked_hint().await {
            Ok(true) => send_event(tx, SessionEvent::ScreenLocked).await,
            Ok(false) => {}
            Err(error) => warn!(%error, "failed to query logind LockedHint"),
        }
    }

    let mut inhibitor = match acquire_sleep_inhibitor(&manager).await {
        Ok(fd) => Some(fd),
        Err(error) => {
            warn!(%error, "failed to acquire logind sleep inhibitor");
            None
        }
    };
    let mut sleep_stream = manager
        .receive_prepare_for_sleep()
        .await
        .context("failed to subscribe to PrepareForSleep")?;
    let mut lock_stream = if let Some(proxy) = session_proxy.as_ref() {
        Some(
            proxy
                .receive_lock()
                .await
                .context("failed to subscribe to Lock")?,
        )
    } else {
        None
    };
    let mut unlock_stream = if let Some(proxy) = session_proxy.as_ref() {
        Some(
            proxy
                .receive_unlock()
                .await
                .context("failed to subscribe to Unlock")?,
        )
    } else {
        None
    };

    loop {
        tokio::select! {
            () = cancel.cancelled() => {
                drop(inhibitor);
                return Ok(RunResult::Cancelled);
            }
            maybe_signal = sleep_stream.next() => {
                let Some(signal) = maybe_signal else {
                    warn!("logind PrepareForSleep stream ended");
                    drop(inhibitor);
                    return Ok(RunResult::Reconnect);
                };

                let args = match signal.args() {
                    Ok(args) => args,
                    Err(error) => {
                        warn!(%error, "failed to decode PrepareForSleep signal; reconnecting logind monitor");
                        drop(inhibitor);
                        return Ok(RunResult::Reconnect);
                    }
                };

                if *args.start() {
                    debug!("logind signalled suspend preparation");
                    send_event(tx, SessionEvent::Suspending).await;

                    tokio::select! {
                        () = cancel.cancelled() => {
                            drop(inhibitor);
                            return Ok(RunResult::Cancelled);
                        }
                        () = tokio::time::sleep(suspend_fade.saturating_add(INHIBITOR_GRACE)) => {}
                    }
                    inhibitor = None;
                } else {
                    debug!("logind signalled resume");
                    send_event(tx, SessionEvent::Resumed).await;
                    inhibitor = match acquire_sleep_inhibitor(&manager).await {
                        Ok(fd) => Some(fd),
                        Err(error) => {
                            warn!(%error, "failed to re-acquire logind sleep inhibitor after resume");
                            None
                        }
                    };
                }
            }
            maybe_signal = next_optional_signal(&mut lock_stream), if lock_stream.is_some() => {
                if maybe_signal.is_none() {
                    lock_stream = None;
                    continue;
                }
                send_event(tx, SessionEvent::ScreenLocked).await;
            }
            maybe_signal = next_optional_signal(&mut unlock_stream), if unlock_stream.is_some() => {
                if maybe_signal.is_none() {
                    unlock_stream = None;
                    continue;
                }
                send_event(tx, SessionEvent::ScreenUnlocked).await;
            }
        }
    }
}

async fn resolve_session_proxy<'a>(
    connection: &'a Connection,
    manager: &LoginManagerProxy<'a>,
) -> anyhow::Result<LoginSessionProxy<'a>> {
    let session_id = env::var("XDG_SESSION_ID").context("XDG_SESSION_ID is not set")?;
    let path = manager
        .get_session(&session_id)
        .await
        .with_context(|| format!("failed to resolve logind session '{session_id}'"))?;

    LoginSessionProxy::builder(connection)
        .path(path)
        .context("invalid logind session path")?
        .build()
        .await
        .context("failed to build logind session proxy")
}

async fn acquire_sleep_inhibitor(manager: &LoginManagerProxy<'_>) -> anyhow::Result<OwnedFd> {
    manager
        .inhibit("sleep", "hypercolor", "Fading LEDs before suspend", "delay")
        .await
        .context("logind sleep inhibitor request failed")
}

async fn send_event(tx: &mpsc::Sender<SessionEvent>, event: SessionEvent) {
    let _ = tx.send(event).await;
}

async fn next_optional_signal<S>(stream: &mut Option<S>) -> Option<S::Item>
where
    S: Stream + Unpin,
{
    match stream {
        Some(stream) => stream.next().await,
        None => None,
    }
}
