//! Session monitoring and power-policy primitives.
//!
//! The session subsystem observes desktop and power-state signals, merges them
//! into a single event stream, and exposes a small policy helper that maps
//! those events onto dim/off/restore actions for the daemon to apply.

mod policy;

#[cfg(target_os = "linux")]
mod logind;

use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

#[cfg(target_os = "linux")]
use self::logind::LogindMonitor;
pub use self::policy::SleepPolicy;

use crate::types::session::{SessionConfig, SessionEvent};

const SESSION_EVENT_CAPACITY: usize = 64;

/// A source of desktop or power-management session events.
#[async_trait]
pub trait SessionMonitor: Send + Sync + 'static {
    /// Human-readable monitor name for tracing.
    fn name(&self) -> &'static str;

    /// Run the monitor until cancellation or a fatal connection error.
    async fn run(
        self,
        tx: mpsc::Sender<SessionEvent>,
        cancel: CancellationToken,
    ) -> anyhow::Result<()>;
}

/// Composite session watcher that merges all available monitor streams.
pub struct SessionWatcher {
    event_tx: broadcast::Sender<SessionEvent>,
    cancel: CancellationToken,
    tasks: Vec<JoinHandle<()>>,
}

#[derive(Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "session dedup tracks four orthogonal state bits"
)]
struct DeduplicationState {
    screen_locked: bool,
    lid_closed: bool,
    idle: bool,
    suspended: bool,
}

impl DeduplicationState {
    fn should_forward(&mut self, event: &SessionEvent) -> bool {
        match event {
            SessionEvent::ScreenLocked => {
                if self.screen_locked {
                    return false;
                }
                self.screen_locked = true;
            }
            SessionEvent::ScreenUnlocked => {
                if !self.screen_locked {
                    return false;
                }
                self.screen_locked = false;
            }
            SessionEvent::Suspending => {
                if self.suspended {
                    return false;
                }
                self.suspended = true;
            }
            SessionEvent::Resumed => {
                if !self.suspended {
                    return false;
                }
                self.suspended = false;
            }
            SessionEvent::IdleEntered { .. } => {
                if self.idle {
                    return false;
                }
                self.idle = true;
            }
            SessionEvent::IdleExited => {
                if !self.idle {
                    return false;
                }
                self.idle = false;
            }
            SessionEvent::LidClosed => {
                if self.lid_closed {
                    return false;
                }
                self.lid_closed = true;
            }
            SessionEvent::LidOpened => {
                if !self.lid_closed {
                    return false;
                }
                self.lid_closed = false;
            }
        }

        true
    }
}

impl SessionWatcher {
    /// Spawn all currently available session monitors.
    pub fn start(config: &SessionConfig) -> Self {
        let (event_tx, _) = broadcast::channel(SESSION_EVENT_CAPACITY);
        let cancel = CancellationToken::new();
        let mut tasks = Vec::new();

        if !config.enabled {
            debug!("Session awareness disabled by config");
            return Self {
                event_tx,
                cancel,
                tasks,
            };
        }

        let (merged_tx, merged_rx) = mpsc::channel(SESSION_EVENT_CAPACITY);
        tasks.push(spawn_forwarder(
            merged_rx,
            event_tx.clone(),
            cancel.child_token(),
        ));

        #[cfg(target_os = "linux")]
        {
            tasks.push(spawn_monitor(
                LogindMonitor::new(config),
                merged_tx,
                cancel.child_token(),
            ));
        }

        if tasks.len() == 1 {
            warn!("Session watcher started without any active session monitors");
        } else {
            debug!(monitor_count = tasks.len() - 1, "Session watcher started");
        }

        Self {
            event_tx,
            cancel,
            tasks,
        }
    }

    /// Subscribe to the merged session event stream.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.event_tx.subscribe()
    }

    /// Shut down all monitors and the merge task.
    pub async fn shutdown(self) {
        self.cancel.cancel();
        for task in self.tasks {
            if let Err(error) = task.await {
                if !error.is_cancelled() {
                    warn!(%error, "Session watcher task terminated unexpectedly");
                }
            }
        }
    }
}

fn spawn_monitor<M: SessionMonitor>(
    monitor: M,
    tx: mpsc::Sender<SessionEvent>,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let monitor_name = monitor.name();
        if let Err(error) = monitor.run(tx, cancel).await {
            warn!(monitor = monitor_name, %error, "Session monitor unavailable");
        }
    })
}

fn spawn_forwarder(
    mut rx: mpsc::Receiver<SessionEvent>,
    tx: broadcast::Sender<SessionEvent>,
    cancel: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut dedup = DeduplicationState::default();
        loop {
            tokio::select! {
                () = cancel.cancelled() => break,
                maybe_event = rx.recv() => {
                    let Some(event) = maybe_event else {
                        break;
                    };

                    if dedup.should_forward(&event) {
                        let _ = tx.send(event);
                    }
                }
            }
        }
    })
}
