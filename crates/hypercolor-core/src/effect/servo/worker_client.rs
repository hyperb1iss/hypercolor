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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use hypercolor_types::canvas::Canvas;

/// Maximum time a `load` call waits for the worker to finish loading the page.
pub(super) const WORKER_READY_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum time an `unload` call waits for the worker to tear the page down.
pub(super) const UNLOAD_TIMEOUT: Duration = Duration::from_secs(8);

/// Opaque handle for a provisioned Servo session on the shared worker thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct ServoSessionId(pub(super) u64);

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
    CreateSession {
        session_id: ServoSessionId,
        width: u32,
        height: u32,
        response_tx: SyncSender<Result<()>>,
    },
    Load {
        session_id: ServoSessionId,
        html_path: PathBuf,
        width: u32,
        height: u32,
        response_tx: SyncSender<Result<()>>,
    },
    LoadUrl {
        session_id: ServoSessionId,
        url: String,
        width: u32,
        height: u32,
        response_tx: SyncSender<Result<()>>,
    },
    #[expect(
        dead_code,
        reason = "explicit unload is kept in the worker protocol for future staged teardown paths"
    )]
    Unload {
        session_id: ServoSessionId,
        response_tx: SyncSender<Result<()>>,
    },
    Render {
        session_id: ServoSessionId,
        scripts: Vec<String>,
        width: u32,
        height: u32,
        response_tx: SyncSender<Result<Canvas>>,
    },
    DestroySession {
        session_id: ServoSessionId,
        response_tx: SyncSender<Result<()>>,
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
    shared: Arc<ServoWorkerClientSharedState>,
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

    fn load(&self) -> WorkerClientState {
        WorkerClientState::from_u8(self.current.load(Ordering::Acquire))
    }

    fn store(&self, state: WorkerClientState) {
        self.current.store(state.as_u8(), Ordering::Release);
    }
}

pub(super) struct ServoWorkerClientSharedState {
    sessions: Mutex<HashMap<ServoSessionId, ClientStateSlot>>,
    next_id: AtomicU64,
}

impl ServoWorkerClientSharedState {
    #[must_use]
    pub(super) fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }
}

impl ServoWorkerClient {
    pub(super) fn new(
        command_tx: Sender<WorkerCommand>,
        shared: Arc<ServoWorkerClientSharedState>,
    ) -> Self {
        Self { command_tx, shared }
    }

    #[cfg(test)]
    pub(super) fn state(&self, session_id: ServoSessionId) -> WorkerClientState {
        self.with_session_slot(session_id, ClientStateSlot::load)
            .expect("test session should exist")
    }

    pub(super) fn create_session_only(&self, width: u32, height: u32) -> Result<ServoSessionId> {
        let session_id = ServoSessionId(self.shared.next_id.fetch_add(1, Ordering::AcqRel));
        self.create_session(session_id, width, height)?;
        Ok(session_id)
    }

    pub(super) fn create_session(
        &self,
        session_id: ServoSessionId,
        width: u32,
        height: u32,
    ) -> Result<()> {
        let inserted = self.with_sessions(|sessions| {
            sessions
                .insert(session_id, ClientStateSlot::new())
                .is_none()
        });
        if !inserted {
            bail!("Servo session {session_id:?} is already registered");
        }

        let (response_tx, response_rx) = mpsc::sync_channel(1);
        if let Err(error) = self.command_tx.send(WorkerCommand::CreateSession {
            session_id,
            width,
            height,
            response_tx,
        }) {
            self.remove_session(session_id);
            return Err(error).context("failed to send create-session command to Servo worker");
        }

        match response_rx.recv_timeout(WORKER_READY_TIMEOUT) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                self.remove_session(session_id);
                Err(error)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.remove_session(session_id);
                bail!(
                    "timed out waiting for Servo session creation after {}ms",
                    WORKER_READY_TIMEOUT.as_millis()
                )
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.remove_session(session_id);
                bail!("Servo worker disconnected before confirming session creation")
            }
        }
    }

    pub(super) fn load_effect(
        &self,
        session_id: ServoSessionId,
        html_path: &Path,
        width: u32,
        height: u32,
    ) -> Result<()> {
        self.transition_to(session_id, WorkerClientState::Loading)?;

        let (response_tx, response_rx) = mpsc::sync_channel(1);
        if let Err(error) = self.command_tx.send(WorkerCommand::Load {
            session_id,
            html_path: html_path.to_path_buf(),
            width,
            height,
            response_tx,
        }) {
            self.transition_to(session_id, WorkerClientState::Idle)?;
            return Err(error).context("failed to send load command to Servo worker");
        }

        match response_rx.recv_timeout(WORKER_READY_TIMEOUT) {
            Ok(Ok(())) => {
                self.transition_to(session_id, WorkerClientState::Running)?;
                Ok(())
            }
            Ok(Err(error)) => {
                self.transition_to(session_id, WorkerClientState::Idle)?;
                Err(error)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.transition_to(session_id, WorkerClientState::Idle)?;
                bail!(
                    "timed out waiting for Servo page load after {}ms",
                    WORKER_READY_TIMEOUT.as_millis()
                )
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.transition_to(session_id, WorkerClientState::Idle)?;
                bail!("Servo worker disconnected before confirming page load")
            }
        }
    }

    pub(super) fn load_url(
        &self,
        session_id: ServoSessionId,
        url: &str,
        width: u32,
        height: u32,
    ) -> Result<()> {
        self.transition_to(session_id, WorkerClientState::Loading)?;

        let (response_tx, response_rx) = mpsc::sync_channel(1);
        if let Err(error) = self.command_tx.send(WorkerCommand::LoadUrl {
            session_id,
            url: url.to_owned(),
            width,
            height,
            response_tx,
        }) {
            self.transition_to(session_id, WorkerClientState::Idle)?;
            return Err(error).context("failed to send load-url command to Servo worker");
        }

        match response_rx.recv_timeout(WORKER_READY_TIMEOUT) {
            Ok(Ok(())) => {
                self.transition_to(session_id, WorkerClientState::Running)?;
                Ok(())
            }
            Ok(Err(error)) => {
                self.transition_to(session_id, WorkerClientState::Idle)?;
                Err(error)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                self.transition_to(session_id, WorkerClientState::Idle)?;
                bail!(
                    "timed out waiting for Servo page load after {}ms",
                    WORKER_READY_TIMEOUT.as_millis()
                )
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.transition_to(session_id, WorkerClientState::Idle)?;
                bail!("Servo worker disconnected before confirming page load")
            }
        }
    }

    pub(super) fn submit_render(
        &self,
        session_id: ServoSessionId,
        scripts: Vec<String>,
        width: u32,
        height: u32,
    ) -> Result<PendingServoFrame> {
        let state = self.with_session_slot(session_id, ClientStateSlot::load)?;
        if state != WorkerClientState::Running {
            bail!("Servo session {session_id:?} is not ready to render (state: {state:?})");
        }

        let (response_tx, response_rx) = mpsc::sync_channel(1);
        self.command_tx
            .send(WorkerCommand::Render {
                session_id,
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

    pub(super) fn destroy_session(&self, session_id: ServoSessionId) -> Result<()> {
        if !self.has_session(session_id) {
            return Ok(());
        }

        let (response_tx, response_rx) = mpsc::sync_channel(1);
        if let Err(error) = self.command_tx.send(WorkerCommand::DestroySession {
            session_id,
            response_tx,
        }) {
            self.remove_session(session_id);
            return Err(error).context("failed to send destroy-session command to Servo worker");
        }

        let result = match response_rx.recv_timeout(UNLOAD_TIMEOUT) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => Err(anyhow::anyhow!(
                "timed out waiting for Servo session destroy after {}ms",
                UNLOAD_TIMEOUT.as_millis()
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(anyhow::anyhow!(
                "Servo worker disconnected before confirming session destroy"
            )),
        };

        self.remove_session(session_id);
        result
    }

    pub(super) fn destroy_session_detached(&self, session_id: ServoSessionId) -> Result<()> {
        if !self.has_session(session_id) {
            return Ok(());
        }

        let (response_tx, _response_rx) = mpsc::sync_channel(1);
        if let Err(error) = self.command_tx.send(WorkerCommand::DestroySession {
            session_id,
            response_tx,
        }) {
            self.remove_session(session_id);
            return Err(error)
                .context("failed to send detached destroy-session command to Servo worker");
        }

        self.remove_session(session_id);
        Ok(())
    }

    fn transition_to(&self, session_id: ServoSessionId, next: WorkerClientState) -> Result<()> {
        self.with_session_slot(session_id, |slot| slot.store(next))
    }

    fn has_session(&self, session_id: ServoSessionId) -> bool {
        self.with_sessions(|sessions| sessions.contains_key(&session_id))
    }

    fn remove_session(&self, session_id: ServoSessionId) {
        self.with_sessions(|sessions| {
            sessions.remove(&session_id);
        });
    }

    fn with_session_slot<R>(
        &self,
        session_id: ServoSessionId,
        f: impl FnOnce(&ClientStateSlot) -> R,
    ) -> Result<R> {
        self.with_sessions(|sessions| {
            sessions
                .get(&session_id)
                .map(f)
                .ok_or_else(|| anyhow::anyhow!("unknown Servo session {session_id:?}"))
        })
    }

    fn with_sessions<R>(
        &self,
        f: impl FnOnce(&mut HashMap<ServoSessionId, ClientStateSlot>) -> R,
    ) -> R {
        match self.shared.sessions.lock() {
            Ok(mut guard) => f(&mut guard),
            Err(poisoned) => f(&mut poisoned.into_inner()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_starts_idle_after_create() {
        let (command_tx, _rx) = mpsc::channel();
        let client =
            ServoWorkerClient::new(command_tx, Arc::new(ServoWorkerClientSharedState::new()));

        let session_id = ServoSessionId(7);
        client.with_sessions(|sessions| {
            sessions.insert(session_id, ClientStateSlot::new());
        });

        assert_eq!(client.state(session_id), WorkerClientState::Idle);
    }

    #[test]
    fn detached_destroy_removes_session_without_waiting_for_worker_reply() {
        let (command_tx, command_rx) = mpsc::channel();
        let client =
            ServoWorkerClient::new(command_tx, Arc::new(ServoWorkerClientSharedState::new()));

        let session_id = ServoSessionId(7);
        client.with_sessions(|sessions| {
            sessions.insert(session_id, ClientStateSlot::new());
        });

        client
            .destroy_session_detached(session_id)
            .expect("detached destroy should queue without waiting");

        assert!(!client.has_session(session_id));
        let WorkerCommand::DestroySession {
            session_id: queued_id,
            ..
        } = command_rx
            .try_recv()
            .expect("detached destroy command should be queued")
        else {
            panic!("expected detached destroy command");
        };
        assert_eq!(queued_id, session_id);
    }
}
