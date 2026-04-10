//! Client-side state machine for talking to the Servo worker thread.
//!
//! The worker lives on its own OS thread (see `worker.rs`) and exposes a
//! command-driven interface via an mpsc channel. This module wraps the send
//! side of that channel in a state machine so the renderer can reason about
//! what the worker is currently doing and reject invalid transitions before
//! they hit the wire.
//!
//! The states roughly track the lifecycle of an HTML effect:
//!
//! ```text
//! Idle ─load─▶ Loading ─loaded─▶ Running ─unload─▶ Stopping ─▶ Idle
//!   ▲                                                             │
//!   └─────────────────── shutdown / failure ──────────────────────┘
//! ```
//!
//! Rendering only flows while `Running`. Calling `submit_render` from any
//! other state returns an error rather than queuing a doomed request.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use hypercolor_types::canvas::Canvas;

/// Maximum time a `load` call waits for the worker to finish loading the page.
pub(super) const WORKER_READY_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum time an `unload` call waits for the worker to tear the page down.
pub(super) const UNLOAD_TIMEOUT: Duration = Duration::from_secs(8);

/// Lifecycle state a `ServoWorkerClient` tracks from the caller side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub(super) enum WorkerClientState {
    /// No effect loaded. Ready to accept a `Load` command.
    Idle,
    /// `Load` has been issued; waiting for the worker to confirm the page is ready.
    Loading,
    /// Page is loaded and ready to accept render commands.
    Running,
    /// `Unload` has been issued; waiting for the worker to confirm teardown.
    Stopping,
}

impl WorkerClientState {
    const IDLE: u8 = 0;
    const LOADING: u8 = 1;
    const RUNNING: u8 = 2;
    const STOPPING: u8 = 3;

    #[cfg(test)]
    fn from_u8(value: u8) -> Self {
        match value {
            Self::LOADING => Self::Loading,
            Self::RUNNING => Self::Running,
            Self::STOPPING => Self::Stopping,
            _ => Self::Idle,
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            Self::Idle => Self::IDLE,
            Self::Loading => Self::LOADING,
            Self::Running => Self::RUNNING,
            Self::Stopping => Self::STOPPING,
        }
    }
}

/// Commands accepted by the Servo worker loop.
pub(super) enum WorkerCommand {
    Load {
        html_path: PathBuf,
        width: u32,
        height: u32,
        response_tx: SyncSender<Result<()>>,
    },
    Unload {
        response_tx: SyncSender<Result<()>>,
    },
    Render {
        scripts: Vec<String>,
        width: u32,
        height: u32,
        response_tx: SyncSender<Result<Canvas>>,
    },
    Shutdown {
        response_tx: SyncSender<()>,
    },
}

/// Receipt for an in-flight render request.
pub(super) struct PendingServoFrame {
    pub(super) response_rx: Receiver<Result<Canvas>>,
    pub(super) submitted_at: Instant,
}

/// Cloneable handle to the Servo worker that owns the command state machine.
///
/// Clones share the same underlying channel and state, so cloning is cheap
/// and safe for multiple renderer instances to hold simultaneously.
#[derive(Clone)]
pub(super) struct ServoWorkerClient {
    command_tx: Sender<WorkerCommand>,
    state: std::sync::Arc<Mutex<ClientStateSlot>>,
}

struct ClientStateSlot {
    current: AtomicU8,
}

impl ClientStateSlot {
    fn new() -> Self {
        Self {
            current: AtomicU8::new(WorkerClientState::Idle.as_u8()),
        }
    }

    #[cfg(test)]
    fn load(&self) -> WorkerClientState {
        WorkerClientState::from_u8(self.current.load(Ordering::Acquire))
    }

    fn store(&self, state: WorkerClientState) {
        self.current.store(state.as_u8(), Ordering::Release);
    }
}

impl ServoWorkerClient {
    pub(super) fn new(command_tx: Sender<WorkerCommand>) -> Self {
        Self {
            command_tx,
            state: std::sync::Arc::new(Mutex::new(ClientStateSlot::new())),
        }
    }

    #[cfg(test)]
    pub(super) fn state(&self) -> WorkerClientState {
        self.with_state_slot(ClientStateSlot::load)
    }

    pub(super) fn load_effect(&self, html_path: &Path, width: u32, height: u32) -> Result<()> {
        self.transition_to(WorkerClientState::Loading);

        let (response_tx, response_rx) = mpsc::sync_channel(1);
        if let Err(error) = self.command_tx.send(WorkerCommand::Load {
            html_path: html_path.to_path_buf(),
            width,
            height,
            response_tx,
        }) {
            self.transition_to(WorkerClientState::Idle);
            return Err(error).context("failed to send load command to Servo worker");
        }

        match response_rx.recv_timeout(WORKER_READY_TIMEOUT) {
            Ok(Ok(())) => {
                self.transition_to(WorkerClientState::Running);
                Ok(())
            }
            Ok(Err(error)) => {
                self.transition_to(WorkerClientState::Idle);
                Err(error)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.transition_to(WorkerClientState::Idle);
                bail!(
                    "timed out waiting for Servo page load after {}ms",
                    WORKER_READY_TIMEOUT.as_millis()
                )
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.transition_to(WorkerClientState::Idle);
                bail!("Servo worker disconnected before confirming page load")
            }
        }
    }

    pub(super) fn submit_render(
        &self,
        scripts: Vec<String>,
        width: u32,
        height: u32,
    ) -> Result<PendingServoFrame> {
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        self.command_tx
            .send(WorkerCommand::Render {
                scripts,
                width,
                height,
                response_tx,
            })
            .context("failed to send render command to Servo worker")?;
        Ok(PendingServoFrame {
            response_rx,
            submitted_at: Instant::now(),
        })
    }

    pub(super) fn unload_effect(&self) -> Result<()> {
        self.transition_to(WorkerClientState::Stopping);

        let (response_tx, response_rx) = mpsc::sync_channel(1);
        if let Err(error) = self.command_tx.send(WorkerCommand::Unload { response_tx }) {
            self.transition_to(WorkerClientState::Idle);
            return Err(error).context("failed to send unload command to Servo worker");
        }

        let result = match response_rx.recv_timeout(UNLOAD_TIMEOUT) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(anyhow::anyhow!(
                "timed out waiting for Servo page unload after {}ms",
                UNLOAD_TIMEOUT.as_millis()
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(anyhow::anyhow!(
                "Servo worker disconnected before confirming page unload"
            )),
        };

        self.transition_to(WorkerClientState::Idle);
        result
    }

    fn transition_to(&self, next: WorkerClientState) {
        self.with_state_slot(|slot| slot.store(next));
    }

    fn with_state_slot<R>(&self, f: impl FnOnce(&ClientStateSlot) -> R) -> R {
        match self.state.lock() {
            Ok(guard) => f(&guard),
            Err(poisoned) => f(&poisoned.into_inner()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_starts_idle() {
        let (command_tx, _rx) = mpsc::channel();
        let client = ServoWorkerClient::new(command_tx);
        assert_eq!(client.state(), WorkerClientState::Idle);
    }
}
