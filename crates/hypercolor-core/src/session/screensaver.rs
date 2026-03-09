//! Linux session-bus screensaver monitor.

use std::time::Duration;

use anyhow::{Context, bail};
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};
use zbus::{Connection, Proxy};

use crate::session::SessionMonitor;
use crate::types::session::SessionEvent;

const RECONNECT_BACKOFF: Duration = Duration::from_secs(1);

const SCREENSAVER_CANDIDATES: &[ScreensaverCandidate] = &[
    ScreensaverCandidate {
        service: "org.freedesktop.ScreenSaver",
        path: "/org/freedesktop/ScreenSaver",
        interface: "org.freedesktop.ScreenSaver",
    },
    ScreensaverCandidate {
        service: "org.gnome.ScreenSaver",
        path: "/org/gnome/ScreenSaver",
        interface: "org.gnome.ScreenSaver",
    },
    ScreensaverCandidate {
        service: "org.mate.ScreenSaver",
        path: "/org/mate/ScreenSaver",
        interface: "org.mate.ScreenSaver",
    },
    ScreensaverCandidate {
        service: "com.canonical.Unity",
        path: "/org/gnome/ScreenSaver",
        interface: "org.gnome.ScreenSaver",
    },
];

#[derive(Clone, Copy)]
struct ScreensaverCandidate {
    service: &'static str,
    path: &'static str,
    interface: &'static str,
}

/// Session-bus monitor for screen blanking/activation signals.
pub struct ScreensaverMonitor;

impl ScreensaverMonitor {
    /// Create a new screensaver monitor.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl SessionMonitor for ScreensaverMonitor {
    fn name(&self) -> &'static str {
        "screensaver"
    }

    async fn run(
        self,
        tx: mpsc::Sender<SessionEvent>,
        cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        let mut connected_once = false;

        loop {
            match run_screensaver_monitor_once(&tx, &cancel).await {
                Ok(RunResult::Cancelled) => break,
                Ok(RunResult::Reconnect) => {
                    connected_once = true;
                    warn!("screensaver monitor connection dropped; rebuilding subscriptions");
                    tokio::select! {
                        () = cancel.cancelled() => break,
                        () = tokio::time::sleep(RECONNECT_BACKOFF) => {}
                    }
                }
                Err(error) => {
                    if !connected_once {
                        return Err(error);
                    }

                    warn!(%error, "screensaver monitor reconnect failed; retrying");
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

async fn run_screensaver_monitor_once(
    tx: &mpsc::Sender<SessionEvent>,
    cancel: &CancellationToken,
) -> anyhow::Result<RunResult> {
    let connection = Connection::session()
        .await
        .context("failed to connect to the session D-Bus")?;
    let (candidate, proxy) = connect_screensaver_proxy(&connection).await?;

    debug!(
        service = candidate.service,
        path = candidate.path,
        interface = candidate.interface,
        "screensaver monitor connected"
    );

    let active: bool = proxy
        .call("GetActive", &())
        .await
        .with_context(|| format!("failed to query GetActive on {}", candidate.service))?;
    if active {
        send_event(tx, SessionEvent::ScreenLocked).await;
    }

    let mut active_stream = proxy
        .receive_signal("ActiveChanged")
        .await
        .with_context(|| {
            format!(
                "failed to subscribe to ActiveChanged on {}",
                candidate.service
            )
        })?;

    loop {
        tokio::select! {
            () = cancel.cancelled() => return Ok(RunResult::Cancelled),
            maybe_signal = active_stream.next() => {
                let Some(signal) = maybe_signal else {
                    warn!(service = candidate.service, "screensaver ActiveChanged stream ended");
                    return Ok(RunResult::Reconnect);
                };

                let active = match signal.body().deserialize::<bool>() {
                    Ok(active) => active,
                    Err(error) => {
                        warn!(
                            service = candidate.service,
                            %error,
                            "failed to decode screensaver ActiveChanged signal; reconnecting monitor"
                        );
                        return Ok(RunResult::Reconnect);
                    }
                };

                let event = if active {
                    SessionEvent::ScreenLocked
                } else {
                    SessionEvent::ScreenUnlocked
                };
                send_event(tx, event).await;
            }
        }
    }
}

async fn connect_screensaver_proxy<'a>(
    connection: &'a Connection,
) -> anyhow::Result<(ScreensaverCandidate, Proxy<'a>)> {
    for candidate in SCREENSAVER_CANDIDATES {
        match Proxy::new(
            connection,
            candidate.service,
            candidate.path,
            candidate.interface,
        )
        .await
        {
            Ok(proxy) => return Ok((*candidate, proxy)),
            Err(error) => {
                debug!(
                    service = candidate.service,
                    path = candidate.path,
                    interface = candidate.interface,
                    %error,
                    "screensaver service unavailable"
                );
            }
        }
    }

    bail!("no supported screensaver service is available on the session bus")
}

async fn send_event(tx: &mpsc::Sender<SessionEvent>, event: SessionEvent) {
    let _ = tx.send(event).await;
}
