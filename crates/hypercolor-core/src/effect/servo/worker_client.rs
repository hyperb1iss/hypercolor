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
//! Idle ─load─▶ Loading ─loaded─▶ Running
//!   ▲                                │
//!   └──── shutdown / failure / destroy ┘
//! ```
//!
//! Rendering only flows while `Running`. Calling `submit_render` from any
//! other state returns an error rather than queuing a doomed request.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::time::{Duration, Instant};

use super::memory::ServoMemoryReportSnapshot;
use crate::effect::traits::EffectRenderOutput;
use anyhow::{Context, Result, bail};
use serde::de::{Deserializer as _, IgnoredAny, MapAccess, Visitor};
use thiserror::Error;

/// Maximum time a `load` call waits for the worker to finish loading the page.
pub(super) const WORKER_READY_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum time an `unload` call waits for the worker to tear the page down.
pub(super) const UNLOAD_TIMEOUT: Duration = Duration::from_secs(8);
pub(super) const SERVO_RENDER_COMMAND_CAPACITY: usize = 256;
pub(super) const SERVO_CONTROL_COMMAND_RESERVE: usize = 64;
pub(super) const SERVO_COMMAND_QUEUE_CAPACITY: usize =
    SERVO_RENDER_COMMAND_CAPACITY + SERVO_CONTROL_COMMAND_RESERVE;

/// Opaque handle for a provisioned Servo session on the shared worker thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct ServoSessionId(pub(super) u64);

/// Producer domain for Servo work that shares one process-global runtime.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum ServoProducerRole {
    #[default]
    SceneHtml,
    DisplayFaceHtml,
}

impl ServoProducerRole {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::SceneHtml => "scene_html",
            Self::DisplayFaceHtml => "display_face_html",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ServoFramePayload {
    json: String,
}

impl ServoFramePayload {
    pub(super) fn from_json(json: String) -> Result<Self> {
        validate_json_object(&json).context("Servo frame payload should be a valid JSON object")?;
        Ok(Self {
            json: escape_js_line_separators(json),
        })
    }

    pub(super) fn as_json(&self) -> &str {
        &self.json
    }

    pub(super) fn len(&self) -> usize {
        self.json.len()
    }
}

fn validate_json_object(json: &str) -> Result<()> {
    struct ObjectVisitor;

    impl<'de> Visitor<'de> for ObjectVisitor {
        type Value = ();

        fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("a JSON object")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
            Ok(())
        }
    }

    let mut deserializer = serde_json::Deserializer::from_str(json);
    deserializer.deserialize_map(ObjectVisitor)?;
    deserializer.end()?;
    Ok(())
}

fn escape_js_line_separators(json: String) -> String {
    if !json.contains(['\u{2028}', '\u{2029}']) {
        return json;
    }

    let mut escaped = String::with_capacity(json.len());
    for character in json.chars() {
        match character {
            '\u{2028}' => escaped.push_str("\\u2028"),
            '\u{2029}' => escaped.push_str("\\u2029"),
            _ => escaped.push(character),
        }
    }
    escaped
}

/// Lifecycle state a `ServoWorkerClient` tracks from the caller side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WorkerClientState {
    /// No effect loaded. Ready to accept a `Load` command.
    Idle,
    /// `Load` has been issued; waiting for the worker to confirm the page is ready.
    Loading,
    /// Page is loaded and ready to accept render commands.
    Running,
}

impl WorkerClientState {
    const IDLE: u8 = 0;
    const LOADING: u8 = 1;
    const RUNNING: u8 = 2;

    fn from_u8(value: u8) -> Self {
        match value {
            Self::LOADING => Self::Loading,
            Self::RUNNING => Self::Running,
            _ => Self::Idle,
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            Self::Idle => Self::IDLE,
            Self::Loading => Self::LOADING,
            Self::Running => Self::RUNNING,
        }
    }
}

/// Commands accepted by the Servo worker loop.
pub(super) enum WorkerCommand {
    CreateSession {
        session_id: ServoSessionId,
        producer_role: ServoProducerRole,
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
    Render {
        session_id: ServoSessionId,
        producer_role: ServoProducerRole,
        scripts: Vec<String>,
        frame_payloads: Vec<ServoFramePayload>,
        width: u32,
        height: u32,
        mode: ServoRenderMode,
        submitted_at: Instant,
        response_tx: SyncSender<Result<EffectRenderOutput>>,
    },
    DestroySession {
        session_id: ServoSessionId,
        response_tx: SyncSender<Result<()>>,
    },
    MemoryReport {
        response_tx: SyncSender<Result<ServoMemoryReportSnapshot>>,
    },
    Shutdown {
        response_tx: SyncSender<()>,
    },
}

#[derive(Debug, Error)]
#[error("Servo worker control queue is saturated while queuing {command}")]
struct ServoControlQueueSaturated {
    command: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ServoRenderMode {
    Cpu,
    #[cfg(feature = "servo-gpu-import")]
    GpuPreferred {
        reuse_cached_on_no_ready: bool,
    },
}

impl ServoRenderMode {
    #[cfg(feature = "servo-gpu-import")]
    pub(super) const fn prefers_gpu(self) -> bool {
        matches!(self, Self::GpuPreferred { .. })
    }

    #[cfg(feature = "servo-gpu-import")]
    pub(super) const fn reuse_cached_gpu_frame_on_no_ready(self) -> bool {
        match self {
            Self::Cpu => false,
            Self::GpuPreferred {
                reuse_cached_on_no_ready,
            } => reuse_cached_on_no_ready,
        }
    }
}

/// Receipt for an in-flight render request.
pub(super) struct PendingServoFrame {
    pub(super) response_rx: Receiver<Result<EffectRenderOutput>>,
    pub(super) submitted_at: Instant,
}

pub(super) enum ServoRenderEnqueue {
    Submitted(PendingServoFrame),
    Saturated,
}

pub(super) struct ServoCommandReservation {
    shared: Arc<ServoWorkerClientSharedState>,
    render: bool,
    active: bool,
}

impl ServoCommandReservation {
    pub(super) fn commit(mut self) {
        self.active = false;
    }
}

impl Drop for ServoCommandReservation {
    fn drop(&mut self) {
        if self.active {
            self.shared.release_command_reservation(self.render);
        }
    }
}

pub(super) struct ServoRenderReservation(ServoCommandReservation);

/// Cloneable handle to the Servo worker that owns the command state machine.
///
/// Clones share the same underlying channel and state, so cloning is cheap
/// and safe for multiple renderer instances to hold simultaneously.
#[derive(Clone)]
pub(super) struct ServoWorkerClient {
    command_tx: SyncSender<WorkerCommand>,
    shared: Arc<ServoWorkerClientSharedState>,
}

struct ClientStateSlot {
    current: AtomicU8,
    producer_role: ServoProducerRole,
}

impl ClientStateSlot {
    fn new(producer_role: ServoProducerRole) -> Self {
        Self {
            current: AtomicU8::new(WorkerClientState::Idle.as_u8()),
            producer_role,
        }
    }

    fn load(&self) -> WorkerClientState {
        WorkerClientState::from_u8(self.current.load(Ordering::Acquire))
    }

    fn store(&self, state: WorkerClientState) {
        self.current.store(state.as_u8(), Ordering::Release);
    }

    const fn producer_role(&self) -> ServoProducerRole {
        self.producer_role
    }
}

pub(super) struct ServoWorkerClientSharedState {
    sessions: Mutex<HashMap<ServoSessionId, ClientStateSlot>>,
    next_id: AtomicU64,
    queued_commands: AtomicUsize,
    queued_render_commands: AtomicUsize,
    deferred_destroys: Mutex<HashSet<ServoSessionId>>,
    deferred_shutdown: Mutex<Option<SyncSender<()>>>,
}

impl ServoWorkerClientSharedState {
    #[must_use]
    pub(super) fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            queued_commands: AtomicUsize::new(0),
            queued_render_commands: AtomicUsize::new(0),
            deferred_destroys: Mutex::new(HashSet::new()),
            deferred_shutdown: Mutex::new(None),
        }
    }

    fn try_reserve_command(self: &Arc<Self>, render: bool) -> Option<ServoCommandReservation> {
        self.queued_commands
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
                (queued < SERVO_COMMAND_QUEUE_CAPACITY).then_some(queued + 1)
            })
            .ok()?;

        if render
            && self
                .queued_render_commands
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
                    (queued < SERVO_RENDER_COMMAND_CAPACITY).then_some(queued + 1)
                })
                .is_err()
        {
            let previous = self.queued_commands.fetch_sub(1, Ordering::AcqRel);
            debug_assert!(previous > 0, "command reservation underflow");
            return None;
        }

        Some(ServoCommandReservation {
            shared: Arc::clone(self),
            render,
            active: true,
        })
    }

    pub(super) fn try_reserve_control_command(self: &Arc<Self>) -> Option<ServoCommandReservation> {
        self.try_reserve_command(false)
    }

    fn try_reserve_render_command(self: &Arc<Self>) -> Option<ServoRenderReservation> {
        self.try_reserve_command(true).map(ServoRenderReservation)
    }

    fn release_command_reservation(&self, render: bool) {
        if render {
            let previous = self.queued_render_commands.fetch_sub(1, Ordering::AcqRel);
            debug_assert!(previous > 0, "render command reservation underflow");
        }
        let previous = self.queued_commands.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0, "command reservation underflow");
    }

    pub(super) fn note_command_dequeued(&self, command: &WorkerCommand) {
        self.release_command_reservation(matches!(command, WorkerCommand::Render { .. }));
    }

    fn defer_destroy(&self, session_id: ServoSessionId) {
        match self.deferred_destroys.lock() {
            Ok(mut deferred) => {
                deferred.insert(session_id);
            }
            Err(poisoned) => {
                poisoned.into_inner().insert(session_id);
            }
        }
    }

    pub(super) fn take_deferred_destroys(&self) -> Vec<ServoSessionId> {
        let deferred = match self.deferred_destroys.lock() {
            Ok(mut deferred) => deferred.drain().collect::<Vec<_>>(),
            Err(poisoned) => poisoned.into_inner().drain().collect::<Vec<_>>(),
        };
        match self.sessions.lock() {
            Ok(mut sessions) => {
                for session_id in &deferred {
                    sessions.remove(session_id);
                }
            }
            Err(poisoned) => {
                let mut sessions = poisoned.into_inner();
                for session_id in &deferred {
                    sessions.remove(session_id);
                }
            }
        }
        deferred
    }

    pub(super) fn defer_shutdown(&self, response_tx: SyncSender<()>) {
        match self.deferred_shutdown.lock() {
            Ok(mut deferred) => *deferred = Some(response_tx),
            Err(poisoned) => *poisoned.into_inner() = Some(response_tx),
        }
    }

    pub(super) fn take_deferred_shutdown(&self) -> Option<SyncSender<()>> {
        match self.deferred_shutdown.lock() {
            Ok(mut deferred) => deferred.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        }
    }
}

impl ServoWorkerClient {
    pub(super) fn new(
        command_tx: SyncSender<WorkerCommand>,
        shared: Arc<ServoWorkerClientSharedState>,
    ) -> Self {
        Self { command_tx, shared }
    }

    #[cfg(test)]
    pub(super) fn state(&self, session_id: ServoSessionId) -> WorkerClientState {
        self.with_session_slot(session_id, ClientStateSlot::load)
            .expect("test session should exist")
    }

    pub(super) fn create_session_only_with_role(
        &self,
        width: u32,
        height: u32,
        producer_role: ServoProducerRole,
    ) -> Result<ServoSessionId> {
        let session_id = ServoSessionId(self.shared.next_id.fetch_add(1, Ordering::AcqRel));
        self.create_session_with_role(session_id, width, height, producer_role)?;
        Ok(session_id)
    }

    pub(super) fn create_session_with_role(
        &self,
        session_id: ServoSessionId,
        width: u32,
        height: u32,
        producer_role: ServoProducerRole,
    ) -> Result<()> {
        let inserted = self.with_sessions(|sessions| {
            sessions
                .insert(session_id, ClientStateSlot::new(producer_role))
                .is_none()
        });
        if !inserted {
            bail!("Servo session {session_id:?} is already registered");
        }

        let (response_tx, response_rx) = mpsc::sync_channel(1);
        if let Err(error) = self.try_send_command(
            WorkerCommand::CreateSession {
                session_id,
                producer_role,
                width,
                height,
                response_tx,
            },
            "create-session",
        ) {
            self.remove_session(session_id);
            return Err(error);
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
        if let Err(error) = self.try_send_command(
            WorkerCommand::Load {
                session_id,
                html_path: html_path.to_path_buf(),
                width,
                height,
                response_tx,
            },
            "load",
        ) {
            self.transition_to(session_id, WorkerClientState::Idle)?;
            return Err(error);
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
        if let Err(error) = self.try_send_command(
            WorkerCommand::LoadUrl {
                session_id,
                url: url.to_owned(),
                width,
                height,
                response_tx,
            },
            "load-url",
        ) {
            self.transition_to(session_id, WorkerClientState::Idle)?;
            return Err(error);
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

    pub(super) fn submit_render_with_payloads_and_mode(
        &self,
        session_id: ServoSessionId,
        scripts: Vec<String>,
        frame_payloads: Vec<ServoFramePayload>,
        width: u32,
        height: u32,
        mode: ServoRenderMode,
    ) -> Result<ServoRenderEnqueue> {
        let Some(reservation) = self.try_reserve_render(session_id)? else {
            return Ok(ServoRenderEnqueue::Saturated);
        };
        self.submit_reserved_render_with_payloads_and_mode(
            reservation,
            session_id,
            scripts,
            frame_payloads,
            width,
            height,
            mode,
        )
        .map(ServoRenderEnqueue::Submitted)
    }

    pub(super) fn try_reserve_render(
        &self,
        session_id: ServoSessionId,
    ) -> Result<Option<ServoRenderReservation>> {
        let state = self.with_session_slot(session_id, ClientStateSlot::load)?;
        if state != WorkerClientState::Running {
            bail!("Servo session {session_id:?} is not ready to render (state: {state:?})");
        }
        Ok(self.shared.try_reserve_render_command())
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the render command carries one coherent worker request"
    )]
    pub(super) fn submit_reserved_render_with_payloads_and_mode(
        &self,
        reservation: ServoRenderReservation,
        session_id: ServoSessionId,
        scripts: Vec<String>,
        frame_payloads: Vec<ServoFramePayload>,
        width: u32,
        height: u32,
        mode: ServoRenderMode,
    ) -> Result<PendingServoFrame> {
        let producer_role = self.with_session_slot(session_id, ClientStateSlot::producer_role)?;
        let submitted_at = Instant::now();
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        let command = WorkerCommand::Render {
            session_id,
            producer_role,
            scripts,
            frame_payloads,
            width,
            height,
            mode,
            submitted_at,
            response_tx,
        };
        match self.command_tx.try_send(command) {
            Ok(()) => {
                reservation.0.commit();
                Ok(PendingServoFrame {
                    response_rx,
                    submitted_at,
                })
            }
            Err(TrySendError::Full(_)) => {
                bail!(
                    "failed to send render command to Servo worker: reserved queue slot was unavailable"
                )
            }
            Err(TrySendError::Disconnected(_)) => {
                bail!("failed to send render command to Servo worker: channel disconnected")
            }
        }
    }

    pub(super) fn destroy_session(&self, session_id: ServoSessionId) -> Result<()> {
        if !self.has_session(session_id) {
            return Ok(());
        }

        let (response_tx, response_rx) = mpsc::sync_channel(1);
        if let Err(error) = self.try_send_command(
            WorkerCommand::DestroySession {
                session_id,
                response_tx,
            },
            "destroy-session",
        ) {
            if error.downcast_ref::<ServoControlQueueSaturated>().is_some() {
                self.shared.defer_destroy(session_id);
            } else {
                self.remove_session(session_id);
            }
            return Err(error);
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
        if let Err(error) = self.try_send_command(
            WorkerCommand::DestroySession {
                session_id,
                response_tx,
            },
            "detached destroy-session",
        ) {
            if error.downcast_ref::<ServoControlQueueSaturated>().is_some() {
                self.shared.defer_destroy(session_id);
            } else {
                self.remove_session(session_id);
            }
            return Err(error);
        }

        self.remove_session(session_id);
        Ok(())
    }

    pub(super) fn memory_report(&self) -> Result<ServoMemoryReportSnapshot> {
        let (response_tx, response_rx) = mpsc::sync_channel(1);
        self.try_send_command(WorkerCommand::MemoryReport { response_tx }, "memory-report")?;

        match response_rx.recv_timeout(WORKER_READY_TIMEOUT) {
            Ok(result) => result,
            Err(mpsc::RecvTimeoutError::Timeout) => bail!(
                "timed out waiting for Servo memory report after {}ms",
                WORKER_READY_TIMEOUT.as_millis()
            ),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                bail!("Servo worker disconnected before returning memory report")
            }
        }
    }

    fn transition_to(&self, session_id: ServoSessionId, next: WorkerClientState) -> Result<()> {
        self.with_session_slot(session_id, |slot| slot.store(next))
    }

    fn try_send_command(&self, command: WorkerCommand, name: &'static str) -> Result<()> {
        let Some(reservation) = self.shared.try_reserve_control_command() else {
            return Err(ServoControlQueueSaturated { command: name }.into());
        };
        match self.command_tx.try_send(command) {
            Ok(()) => {
                reservation.commit();
                Ok(())
            }
            Err(TrySendError::Full(_)) => Err(ServoControlQueueSaturated { command: name }.into()),
            Err(TrySendError::Disconnected(_)) => {
                bail!("failed to send {name} command to Servo worker: channel disconnected")
            }
        }
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
        let (command_tx, _rx) = mpsc::sync_channel(1);
        let client =
            ServoWorkerClient::new(command_tx, Arc::new(ServoWorkerClientSharedState::new()));

        let session_id = ServoSessionId(7);
        client.with_sessions(|sessions| {
            sessions.insert(
                session_id,
                ClientStateSlot::new(ServoProducerRole::SceneHtml),
            );
        });

        assert_eq!(client.state(session_id), WorkerClientState::Idle);
    }

    #[test]
    fn detached_destroy_removes_session_without_waiting_for_worker_reply() {
        let (command_tx, command_rx) = mpsc::sync_channel(1);
        let client =
            ServoWorkerClient::new(command_tx, Arc::new(ServoWorkerClientSharedState::new()));

        let session_id = ServoSessionId(7);
        client.with_sessions(|sessions| {
            sessions.insert(
                session_id,
                ClientStateSlot::new(ServoProducerRole::SceneHtml),
            );
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

    #[test]
    fn render_submission_retains_work_without_blocking_when_command_queue_is_full() {
        let (command_tx, _command_rx) = mpsc::sync_channel(1);
        let shared = Arc::new(ServoWorkerClientSharedState::new());
        let client = ServoWorkerClient::new(command_tx, Arc::clone(&shared));
        let session_id = ServoSessionId(7);
        client.with_sessions(|sessions| {
            let slot = ClientStateSlot::new(ServoProducerRole::SceneHtml);
            slot.store(WorkerClientState::Running);
            sessions.insert(session_id, slot);
        });
        let reservations = (0..SERVO_RENDER_COMMAND_CAPACITY)
            .map(|_| {
                shared
                    .try_reserve_render_command()
                    .expect("test should reserve the render budget")
            })
            .collect::<Vec<_>>();

        let result = client
            .submit_render_with_payloads_and_mode(
                session_id,
                vec!["window.tick()".to_owned()],
                Vec::new(),
                320,
                200,
                ServoRenderMode::Cpu,
            )
            .expect("queue saturation should be recoverable");

        let ServoRenderEnqueue::Saturated = result else {
            panic!("a full command queue must report saturation without blocking");
        };
        assert_eq!(client.state(session_id), WorkerClientState::Running);
        drop(reservations);
    }

    #[test]
    fn render_admission_preserves_control_queue_capacity() {
        let shared = Arc::new(ServoWorkerClientSharedState::new());
        let render_reservations = (0..SERVO_RENDER_COMMAND_CAPACITY)
            .map(|_| {
                shared
                    .try_reserve_render_command()
                    .expect("render reservation")
            })
            .collect::<Vec<_>>();
        let control_reservations = (0..SERVO_CONTROL_COMMAND_RESERVE)
            .map(|_| {
                shared
                    .try_reserve_control_command()
                    .expect("reserved control capacity")
            })
            .collect::<Vec<_>>();

        assert!(shared.try_reserve_render_command().is_none());
        assert!(shared.try_reserve_control_command().is_none());

        drop(control_reservations);
        assert!(shared.try_reserve_control_command().is_some());
        drop(render_reservations);
    }

    #[test]
    fn saturated_destroy_is_deferred_without_poisoning_or_forgetting_session() {
        let (command_tx, _command_rx) = mpsc::sync_channel(1);
        let fill_tx = command_tx.clone();
        let shared = Arc::new(ServoWorkerClientSharedState::new());
        let client = ServoWorkerClient::new(command_tx, Arc::clone(&shared));
        let session_id = ServoSessionId(9);
        client.with_sessions(|sessions| {
            sessions.insert(
                session_id,
                ClientStateSlot::new(ServoProducerRole::SceneHtml),
            );
        });
        let (response_tx, _response_rx) = mpsc::sync_channel(1);
        fill_tx
            .try_send(WorkerCommand::MemoryReport { response_tx })
            .expect("test command should fill the bounded queue");

        let error = client
            .destroy_session_detached(session_id)
            .expect_err("saturated control queue should defer cleanup");

        assert!(error.downcast_ref::<ServoControlQueueSaturated>().is_some());
        assert!(!super::super::worker::servo_worker_is_fatal_error(&error));
        assert!(client.has_session(session_id));
        assert_eq!(shared.take_deferred_destroys(), [session_id]);
        assert!(!client.has_session(session_id));
    }
}
