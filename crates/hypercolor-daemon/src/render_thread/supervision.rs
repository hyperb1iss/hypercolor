//! Shared health and recovery state for the supervised render thread.
//!
//! The render thread restarts its pipeline in-process after a panic or an
//! error exit, within a bounded budget, and asks the daemon to exit once the
//! budget runs out or the compositor GPU device is lost. Everything the rest
//! of the daemon needs to see about that lives here: the lifecycle phase, a
//! heartbeat and frame-completion clock the render loop stamps every
//! iteration, restart counters, the last failure, and the termination
//! request the daemon waits on.
//!
//! Failures are also persisted to `render-failure.json` in the daemon state
//! directory, so the record survives a log that was lost (a full tmpfs, a
//! closed pipe) and a `panic = "abort"` build that never returns from the
//! panic hook. The next daemon run loads the previous record and reports it.

use std::collections::VecDeque;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError, Weak};
use std::thread::ThreadId;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

#[cfg(feature = "persistence-test-hooks")]
use super::gpu_errors::DeviceLossSignal;
use super::gpu_errors::{GpuErrorTotals, gpu_error_totals};
use crate::panic_hook::{PanicObserver, PanicReport};

/// Name of the OS thread that runs the render pipeline.
pub const RENDER_THREAD_NAME: &str = "hypercolor-render";

/// Name of the render runtime's Tokio worker threads.
pub const RENDER_RUNTIME_THREAD_NAME: &str = "hypercolor-render-rt";

/// File in the daemon state directory that holds the last render failure.
pub const RENDER_FAILURE_FILE: &str = "render-failure.json";

/// Identity of this daemon process, written into every failure record.
///
/// A pid cannot tell runs apart: a container restarts the daemon as pid 1
/// every time, and pids are reused across boots. This id is unique per
/// process, so a record from an earlier run is always recognized as one.
static PROCESS_INSTANCE: LazyLock<String> = LazyLock::new(|| uuid::Uuid::now_v7().to_string());

/// How long a running render loop may go without completing an iteration
/// before the daemon reports itself unhealthy to its service manager's
/// watchdog. A healthy loop iterates at least every quarter second, so this
/// only trips on a thread that is wedged inside a frame, which nothing can
/// interrupt in-process.
pub const RENDER_WEDGED_AFTER: Duration = Duration::from_secs(20);

/// Whether `name` belongs to the render thread or its runtime workers.
#[must_use]
pub fn is_render_thread_name(name: &str) -> bool {
    name == RENDER_THREAD_NAME || name.starts_with(RENDER_RUNTIME_THREAD_NAME)
}

// ── Policy ──────────────────────────────────────────────────────────────────

/// How often and how fast a failed render pipeline restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderRestartPolicy {
    /// Restarts allowed within [`window`](Self::window). The failure after
    /// the last allowed restart terminates the daemon instead.
    pub max_restarts: u32,
    /// Rolling window the restart budget is counted over.
    pub window: Duration,
    /// Delay before the first restart in a streak.
    pub initial_backoff: Duration,
    /// Upper bound on the doubling restart delay.
    pub max_backoff: Duration,
    /// A pipeline that runs this long before failing starts a fresh streak,
    /// so its restart waits the initial backoff again.
    pub stable_uptime: Duration,
}

impl Default for RenderRestartPolicy {
    fn default() -> Self {
        Self {
            max_restarts: 5,
            window: Duration::from_secs(300),
            initial_backoff: Duration::from_millis(250),
            max_backoff: Duration::from_secs(8),
            stable_uptime: Duration::from_secs(60),
        }
    }
}

impl RenderRestartPolicy {
    fn backoff(&self, consecutive_failures: u32) -> Duration {
        let doublings = consecutive_failures.saturating_sub(1).min(31);
        self.initial_backoff
            .saturating_mul(1_u32 << doublings)
            .min(self.max_backoff)
    }
}

/// What the supervisor does after a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryPlan {
    /// Rebuild the pipeline after `backoff`; `attempt` counts restarts in
    /// the current window.
    Restart { attempt: u32, backoff: Duration },
    /// Stop rendering and ask the daemon to exit.
    Terminate(TerminationCause),
    /// The daemon is already stopping, so nothing restarts.
    DaemonStopping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminationCause {
    RestartBudgetExhausted {
        failures: u32,
    },
    GpuDeviceLost,
    /// The supervisor itself failed outside a pipeline attempt.
    SupervisorFailed,
}

/// Rolling restart budget for one render thread.
#[derive(Debug)]
pub(crate) struct RestartBudget {
    policy: RenderRestartPolicy,
    failures: VecDeque<Instant>,
    consecutive: u32,
}

impl RestartBudget {
    pub(crate) const fn new(policy: RenderRestartPolicy) -> Self {
        Self {
            policy,
            failures: VecDeque::new(),
            consecutive: 0,
        }
    }

    /// Account for a failure at `now` of an attempt that had run for
    /// `uptime`, and decide whether to restart.
    pub(crate) fn record(&mut self, now: Instant, uptime: Duration) -> RecoveryPlan {
        if uptime >= self.policy.stable_uptime {
            self.consecutive = 0;
        }
        self.consecutive = self.consecutive.saturating_add(1);
        self.failures.push_back(now);
        while self
            .failures
            .front()
            .is_some_and(|failed| now.saturating_duration_since(*failed) > self.policy.window)
        {
            self.failures.pop_front();
        }
        let failures = u32::try_from(self.failures.len()).unwrap_or(u32::MAX);
        if failures > self.policy.max_restarts {
            return RecoveryPlan::Terminate(TerminationCause::RestartBudgetExhausted { failures });
        }
        RecoveryPlan::Restart {
            attempt: failures,
            backoff: self.policy.backoff(self.consecutive),
        }
    }

    pub(crate) const fn consecutive(&self) -> u32 {
        self.consecutive
    }
}

// ── Records ─────────────────────────────────────────────────────────────────

/// Where the render thread is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RenderThreadPhase {
    /// No render thread has been spawned for this supervision.
    NotStarted = 0,
    /// The thread is building its runtime, input pump, and first pipeline.
    Starting = 1,
    /// A pipeline is running.
    Running = 2,
    /// A pipeline failed and the supervisor is rebuilding it.
    Restarting = 3,
    /// The supervisor gave up; the daemon is exiting so its service manager
    /// restarts it.
    Terminating = 4,
    /// The thread exited cleanly (daemon shutdown or a loop never started).
    Stopped = 5,
}

impl RenderThreadPhase {
    const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Starting,
            2 => Self::Running,
            3 => Self::Restarting,
            4 => Self::Terminating,
            5 => Self::Stopped,
            _ => Self::NotStarted,
        }
    }

    /// Stable snake-case name used by diagnostics and status.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotStarted => "not_started",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Restarting => "restarting",
            Self::Terminating => "terminating",
            Self::Stopped => "stopped",
        }
    }
}

/// What ended a render pipeline run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderFailureKind {
    /// The pipeline panicked.
    Panic,
    /// The input publication pump the pipeline reads from stopped.
    InputPumpTerminated,
    /// A replacement input publication pump failed to start.
    InputPumpRestartFailed,
    /// A replacement pipeline failed to build.
    PipelineRebuildFailed,
    /// The compositor GPU device was lost.
    GpuDeviceLost,
}

impl RenderFailureKind {
    /// Stable snake-case name used by diagnostics and status.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Panic => "panic",
            Self::InputPumpTerminated => "input_pump_terminated",
            Self::InputPumpRestartFailed => "input_pump_restart_failed",
            Self::PipelineRebuildFailed => "pipeline_rebuild_failed",
            Self::GpuDeviceLost => "gpu_device_lost",
        }
    }
}

/// What the daemon did about a render failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderRecoveryAction {
    /// The pipeline was rebuilt in-process with fresh GPU and render state.
    Restart,
    /// The daemon exits with an error so its service manager restarts it.
    TerminateProcess,
    /// The daemon was already stopping, so nothing was restarted.
    DaemonStopping,
    /// Written by the panic hook in a `panic = "abort"` build, which aborts
    /// the process right after.
    ProcessAbort,
    /// Written by the panic hook before the supervisor caught the unwind.
    /// The supervisor replaces it with its decision; a record still in
    /// this state means the supervisor never got control back.
    AwaitingSupervisor,
}

impl RenderRecoveryAction {
    /// Stable snake-case name used by diagnostics and status.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Restart => "restart",
            Self::TerminateProcess => "terminate_process",
            Self::DaemonStopping => "daemon_stopping",
            Self::ProcessAbort => "process_abort",
            Self::AwaitingSupervisor => "awaiting_supervisor",
        }
    }
}

/// One render failure, as logged and persisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderFailureRecord {
    pub kind: RenderFailureKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// RFC 3339 UTC time of the failure.
    pub occurred_at: String,
    pub occurred_at_unix_ms: u64,
    /// Process that recorded the failure.
    pub pid: u32,
    /// Unique id of the daemon process that recorded the failure. Absent
    /// from records written before it existed.
    #[serde(default)]
    pub process_instance: String,
    pub daemon_version: String,
    /// Restarts in the budget window including the one this failure
    /// triggered, or the failures counted when the budget ran out.
    pub restart_attempt: u32,
    /// Restarts since the process started, after this failure.
    pub restarts_total: u64,
    pub action: RenderRecoveryAction,
    /// Backtrace frames from the panic site, when the failure was a panic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backtrace: Option<String>,
}

/// A failure before the supervisor decides what to do with it.
#[derive(Debug, Clone)]
pub(crate) struct RenderFailure {
    pub(crate) kind: RenderFailureKind,
    pub(crate) message: String,
    pub(crate) thread: Option<String>,
    pub(crate) location: Option<String>,
    pub(crate) backtrace: Option<String>,
}

impl RenderFailure {
    pub(crate) fn panic(payload: String, report: Option<PanicReport>) -> Self {
        let thread = std::thread::current().name().map(str::to_owned);
        match report {
            Some(report) => Self {
                kind: RenderFailureKind::Panic,
                message: report.payload.clone(),
                thread: Some(report.thread_name.clone()),
                location: report.location.clone(),
                backtrace: Some(report.backtrace_excerpt()),
            },
            None => Self {
                kind: RenderFailureKind::Panic,
                message: payload,
                thread,
                location: None,
                backtrace: None,
            },
        }
    }

    /// Attach a panic report's thread, location, and backtrace, keeping
    /// this failure's kind and message.
    pub(crate) fn with_panic_report(mut self, report: Option<PanicReport>) -> Self {
        if let Some(report) = report {
            self.message = format!(
                "{} (after a panic on {}: {})",
                self.message, report.thread_name, report.payload
            );
            self.thread = Some(report.thread_name.clone());
            self.location.clone_from(&report.location);
            self.backtrace = Some(report.backtrace_excerpt());
        }
        self
    }

    pub(crate) fn from_error(kind: RenderFailureKind, error: &anyhow::Error) -> Self {
        Self::new(kind, format!("{error:#}"))
    }

    pub(crate) fn new(kind: RenderFailureKind, message: String) -> Self {
        Self {
            kind,
            message,
            thread: std::thread::current().name().map(str::to_owned),
            location: None,
            backtrace: None,
        }
    }

    /// Reclassify a failure that happened after the compositor device was
    /// lost: a lost device is why it failed, and why it cannot restart.
    pub(crate) fn after_device_loss(mut self, lost: &str) -> Self {
        self.message = format!(
            "{lost}; the pipeline then failed with {}: {}",
            self.kind.as_str(),
            self.message
        );
        self.kind = RenderFailureKind::GpuDeviceLost;
        self
    }
}

/// Everything diagnostics and status read about the render thread.
#[derive(Debug, Clone)]
pub struct RenderSupervisionSnapshot {
    pub phase: RenderThreadPhase,
    pub restarts_total: u64,
    /// Failures since the pipeline last ran past the stable uptime.
    pub consecutive_failures: u32,
    pub max_restarts: u32,
    /// Time since the render loop last completed an iteration of any kind,
    /// paused polls included.
    pub last_heartbeat_age: Option<Duration>,
    /// Time since the render loop last completed a frame.
    pub last_frame_age: Option<Duration>,
    /// How long the current pipeline has been running.
    pub current_run: Option<Duration>,
    /// The latest failure in this process, or else the failure the previous
    /// daemon run persisted.
    pub last_failure: Option<RenderFailureRecord>,
    /// Whether [`last_failure`](Self::last_failure) happened in this process.
    pub last_failure_in_this_process: bool,
    pub gpu_errors: GpuErrorTotals,
}

impl RenderSupervisionSnapshot {
    /// The API projection of this snapshot.
    ///
    /// `include_private` gates the failure message and the last GPU error
    /// text, which can carry source paths and resource labels.
    #[must_use]
    pub fn status(
        &self,
        include_private: bool,
    ) -> hypercolor_types::api::system::RenderSupervisionStatus {
        use hypercolor_types::api::system::{
            GpuErrorStatus, RenderFailureStatus, RenderSupervisionStatus,
        };

        RenderSupervisionStatus {
            phase: self.phase.as_str().to_owned(),
            restarts_total: self.restarts_total,
            consecutive_failures: self.consecutive_failures,
            max_restarts: self.max_restarts,
            last_frame_age_ms: self.last_frame_age.map(duration_millis),
            last_failure: self
                .last_failure
                .as_ref()
                .map(|record| RenderFailureStatus {
                    kind: record.kind.as_str().to_owned(),
                    message: if include_private {
                        record.message.clone()
                    } else {
                        String::new()
                    },
                    thread: record.thread.clone(),
                    location: include_private.then(|| record.location.clone()).flatten(),
                    occurred_at: record.occurred_at.clone(),
                    action: record.action.as_str().to_owned(),
                    restart_attempt: record.restart_attempt,
                    current_process: self.last_failure_in_this_process,
                }),
            gpu_errors: GpuErrorStatus {
                validation_errors: self.gpu_errors.validation,
                out_of_memory_errors: self.gpu_errors.out_of_memory,
                internal_errors: self.gpu_errors.internal,
                device_lost: self.gpu_errors.device_lost,
                last_error: include_private
                    .then(|| {
                        self.gpu_errors
                            .last
                            .as_ref()
                            .map(|last| format!("{}: {}: {}", last.device, last.kind, last.message))
                    })
                    .flatten(),
            },
        }
    }
}

fn duration_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

// ── Persistence ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct RenderFailureStore {
    path: PathBuf,
}

/// Prefix and suffix of a record staged before its durable replace.
const STAGING_PREFIX: &str = ".render-failure.";
const STAGING_SUFFIX: &str = ".tmp";

impl RenderFailureStore {
    /// Remove records a killed process staged and never replaced.
    fn remove_stale_staging(&self) {
        let Some(directory) = self.path.parent() else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.filter_map(std::result::Result::ok) {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(STAGING_PREFIX) && name.ends_with(STAGING_SUFFIX) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    fn load(&self) -> Option<RenderFailureRecord> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => {
                tracing::warn!(path = %self.path.display(), %error, "could not read the previous render failure record");
                return None;
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(record) => Some(record),
            Err(error) => {
                tracing::warn!(path = %self.path.display(), %error, "ignoring an unreadable render failure record");
                None
            }
        }
    }

    /// Replace the record durably. Safe to call from the panic hook: it
    /// takes no lock the render thread could be holding.
    fn persist(&self, record: &RenderFailureRecord) -> Result<()> {
        let directory = self
            .path
            .parent()
            .context("render failure record path has no parent directory")?;
        std::fs::create_dir_all(directory).with_context(|| {
            format!(
                "failed to create the render failure directory {}",
                directory.display()
            )
        })?;
        let payload = serde_json::to_vec_pretty(record)
            .context("failed to serialize the render failure record")?;
        let mut staged = tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .suffix(STAGING_SUFFIX)
            .tempfile_in(directory)
            .context("failed to stage the render failure record")?;
        staged
            .write_all(&payload)
            .and_then(|()| staged.as_file().sync_all())
            .context("failed to write the staged render failure record")?;
        let (_file, staged_path) = staged
            .keep()
            .context("failed to keep the staged render failure record")?;
        if let Err(error) = hypercolor_platform_fs::durable_replace(&staged_path, &self.path) {
            let _ = std::fs::remove_file(&staged_path);
            return Err(error).with_context(|| {
                format!(
                    "failed to replace the render failure record {}",
                    self.path.display()
                )
            });
        }
        Ok(())
    }
}

// ── Supervision handle ──────────────────────────────────────────────────────

/// A fault the render thread raises on itself, for tests of the recovery
/// paths.
#[cfg(feature = "persistence-test-hooks")]
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectedRenderFault {
    /// Panic with this message at the top of the next render loop
    /// iteration.
    Panic(&'static str),
    /// Block the render thread for the given time, like a wedged frame.
    Stall(Duration),
    /// Raise one uncaptured validation error on the compositor GPU device.
    GpuValidationError,
}

struct SupervisionInner {
    policy: RenderRestartPolicy,
    store: Option<RenderFailureStore>,
    epoch: Instant,
    phase: AtomicU8,
    restarts_total: AtomicU64,
    consecutive_failures: AtomicU32,
    attempt_started_us: AtomicU64,
    last_heartbeat_us: AtomicU64,
    last_frame_us: AtomicU64,
    last_failure: Mutex<Option<RenderFailureRecord>>,
    previous_failure: Option<RenderFailureRecord>,
    termination: watch::Sender<Option<Arc<RenderFailureRecord>>>,
    /// The `hypercolor-render` thread this supervision watches, so its
    /// panic observer persists only that thread's panics.
    render_thread: Mutex<Option<ThreadId>>,
    #[cfg(feature = "persistence-test-hooks")]
    injected_device_loss: DeviceLossSignal,
    #[cfg(feature = "persistence-test-hooks")]
    injected_faults: Mutex<VecDeque<InjectedRenderFault>>,
    #[cfg(feature = "persistence-test-hooks")]
    injected_fault_pending: std::sync::atomic::AtomicBool,
}

/// Shared handle to one render thread's health and recovery state.
///
/// Cloning shares the same state. The daemon creates one at startup and
/// hands it to the render thread, diagnostics, and status.
#[derive(Clone)]
pub struct RenderSupervision {
    inner: Arc<SupervisionInner>,
}

impl std::fmt::Debug for RenderSupervision {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RenderSupervision")
            .field("phase", &self.phase())
            .field("policy", &self.inner.policy)
            .finish_non_exhaustive()
    }
}

impl Default for RenderSupervision {
    fn default() -> Self {
        Self::detached()
    }
}

impl RenderSupervision {
    /// Supervision with `policy` that persists failures to
    /// `failure_record_path` when one is given.
    ///
    /// A record left by an earlier process at that path is loaded and
    /// reported until this process records its own failure.
    #[must_use]
    pub fn new(policy: RenderRestartPolicy, failure_record_path: Option<PathBuf>) -> Self {
        let store = failure_record_path.map(|path| RenderFailureStore { path });
        if let Some(store) = store.as_ref() {
            store.remove_stale_staging();
        }
        let previous_failure = store
            .as_ref()
            .and_then(RenderFailureStore::load)
            .filter(|record| record.process_instance != *PROCESS_INSTANCE);
        if let Some(record) = previous_failure.as_ref() {
            tracing::warn!(
                kind = record.kind.as_str(),
                message = %record.message,
                occurred_at = %record.occurred_at,
                pid = record.pid,
                action = record.action.as_str(),
                "an earlier daemon run recorded a render failure"
            );
        }
        Self {
            inner: Arc::new(SupervisionInner {
                policy,
                store,
                epoch: Instant::now(),
                phase: AtomicU8::new(RenderThreadPhase::NotStarted as u8),
                restarts_total: AtomicU64::new(0),
                consecutive_failures: AtomicU32::new(0),
                attempt_started_us: AtomicU64::new(0),
                last_heartbeat_us: AtomicU64::new(0),
                last_frame_us: AtomicU64::new(0),
                last_failure: Mutex::new(None),
                previous_failure,
                termination: watch::Sender::new(None),
                render_thread: Mutex::new(None),
                #[cfg(feature = "persistence-test-hooks")]
                injected_device_loss: DeviceLossSignal::default(),
                #[cfg(feature = "persistence-test-hooks")]
                injected_faults: Mutex::new(VecDeque::new()),
                #[cfg(feature = "persistence-test-hooks")]
                injected_fault_pending: std::sync::atomic::AtomicBool::new(false),
            }),
        }
    }

    /// Default policy, persisting to [`RENDER_FAILURE_FILE`] in
    /// `state_dir`.
    #[must_use]
    pub fn for_state_dir(state_dir: &Path) -> Self {
        Self::new(
            RenderRestartPolicy::default(),
            Some(state_dir.join(RENDER_FAILURE_FILE)),
        )
    }

    /// Default policy without persistence.
    #[must_use]
    pub fn detached() -> Self {
        Self::new(RenderRestartPolicy::default(), None)
    }

    #[must_use]
    pub fn policy(&self) -> RenderRestartPolicy {
        self.inner.policy
    }

    /// Where failures are persisted, if anywhere.
    #[must_use]
    pub fn failure_record_path(&self) -> Option<&Path> {
        self.inner.store.as_ref().map(|store| store.path.as_path())
    }

    #[must_use]
    pub fn phase(&self) -> RenderThreadPhase {
        RenderThreadPhase::from_u8(self.inner.phase.load(Ordering::Acquire))
    }

    /// Read every health field at once.
    #[must_use]
    pub fn snapshot(&self) -> RenderSupervisionSnapshot {
        let now = Instant::now();
        let failure_guard = self
            .inner
            .last_failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let current = failure_guard.clone();
        let phase = self.phase();
        let restarts_total = self.inner.restarts_total.load(Ordering::Relaxed);
        let consecutive_failures = self.inner.consecutive_failures.load(Ordering::Relaxed);
        drop(failure_guard);
        let last_failure_in_this_process = current.is_some();
        RenderSupervisionSnapshot {
            phase,
            restarts_total,
            consecutive_failures,
            max_restarts: self.inner.policy.max_restarts,
            last_heartbeat_age: self.age(&self.inner.last_heartbeat_us, now),
            last_frame_age: self.age(&self.inner.last_frame_us, now),
            current_run: (phase == RenderThreadPhase::Running)
                .then(|| self.age(&self.inner.attempt_started_us, now))
                .flatten(),
            last_failure: current.or_else(|| self.inner.previous_failure.clone()),
            last_failure_in_this_process,
            gpu_errors: gpu_error_totals(),
        }
    }

    /// Whether the render thread is healthy enough for a service manager
    /// watchdog: `false` only while a running pipeline has gone
    /// [`RENDER_WEDGED_AFTER`] without completing a loop iteration.
    #[must_use]
    pub fn watchdog_healthy(&self) -> bool {
        self.watchdog_healthy_at(Instant::now())
    }

    fn watchdog_healthy_at(&self, now: Instant) -> bool {
        self.phase() != RenderThreadPhase::Running
            || self
                .age(&self.inner.last_heartbeat_us, now)
                .is_none_or(|age| age < RENDER_WEDGED_AFTER)
    }

    /// The failure the render thread terminated on, once it gave up.
    #[must_use]
    pub fn termination(&self) -> Option<Arc<RenderFailureRecord>> {
        self.inner.termination.borrow().clone()
    }

    /// Wait until the render thread gives up, and return why.
    pub async fn terminated(&self) -> Arc<RenderFailureRecord> {
        let mut receiver = self.inner.termination.subscribe();
        let record = receiver
            .wait_for(Option::is_some)
            .await
            .map(|record| record.clone());
        match record {
            Ok(Some(record)) => record,
            // The sender lives as long as this handle, so the wait cannot
            // end any other way.
            Ok(None) | Err(_) => std::future::pending().await,
        }
    }

    /// Bind this supervision to the render thread calling it.
    pub(crate) fn bind_render_thread(&self) {
        *self
            .inner
            .render_thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(std::thread::current().id());
    }

    /// The loss a test injected for this supervision's compositor device.
    #[cfg(feature = "persistence-test-hooks")]
    pub(crate) fn injected_device_loss(&self) -> DeviceLossSignal {
        self.inner.injected_device_loss.clone()
    }

    pub(crate) fn set_phase(&self, phase: RenderThreadPhase) {
        self.inner.phase.store(phase as u8, Ordering::Release);
    }

    /// A pipeline attempt starts running at `now`.
    pub(crate) fn attempt_started(&self, now: Instant) {
        let stamp = self.stamp(now);
        self.inner
            .attempt_started_us
            .store(stamp, Ordering::Relaxed);
        // A fresh pipeline gets the same grace a fresh frame would, so the
        // stall check measures this attempt rather than the dead one.
        self.inner.last_heartbeat_us.store(stamp, Ordering::Relaxed);
        self.inner.last_frame_us.store(stamp, Ordering::Relaxed);
        self.set_phase(RenderThreadPhase::Running);
    }

    /// The render loop completed an idle or paused iteration at `now`.
    pub(crate) fn heartbeat(&self, now: Instant) {
        self.inner
            .last_heartbeat_us
            .store(self.stamp(now), Ordering::Relaxed);
    }

    /// The render loop completed a frame at `now`.
    pub(crate) fn frame_completed(&self, now: Instant) {
        let stamp = self.stamp(now);
        self.inner.last_heartbeat_us.store(stamp, Ordering::Relaxed);
        self.inner.last_frame_us.store(stamp, Ordering::Relaxed);
        if self.inner.consecutive_failures.load(Ordering::Relaxed) != 0 {
            let started = self.inner.attempt_started_us.load(Ordering::Relaxed);
            let stable_us =
                u64::try_from(self.inner.policy.stable_uptime.as_micros()).unwrap_or(u64::MAX);
            if stamp.saturating_sub(started) >= stable_us {
                self.inner.consecutive_failures.store(0, Ordering::Relaxed);
            }
        }
    }

    /// Record `failure` and the plan for it: log it, persist it, update the
    /// counters and the phase.
    pub(crate) fn record_failure(
        &self,
        failure: RenderFailure,
        plan: RecoveryPlan,
        consecutive_failures: u32,
    ) -> RenderFailureRecord {
        let (action, restart_attempt, phase) = match plan {
            RecoveryPlan::Restart { attempt, .. } => (
                RenderRecoveryAction::Restart,
                attempt,
                RenderThreadPhase::Restarting,
            ),
            RecoveryPlan::Terminate(TerminationCause::RestartBudgetExhausted { failures }) => (
                RenderRecoveryAction::TerminateProcess,
                failures,
                RenderThreadPhase::Terminating,
            ),
            RecoveryPlan::Terminate(
                TerminationCause::GpuDeviceLost | TerminationCause::SupervisorFailed,
            ) => (
                RenderRecoveryAction::TerminateProcess,
                consecutive_failures,
                RenderThreadPhase::Terminating,
            ),
            RecoveryPlan::DaemonStopping => (
                RenderRecoveryAction::DaemonStopping,
                consecutive_failures,
                RenderThreadPhase::Stopped,
            ),
        };
        // Only the render thread records failures, so the counter cannot
        // move between this read and the commit below.
        let previous_restarts = self.inner.restarts_total.load(Ordering::Relaxed);
        let restarts_total = if action == RenderRecoveryAction::Restart {
            previous_restarts.saturating_add(1)
        } else {
            previous_restarts
        };
        let record = failure_record(failure, action, restart_attempt, restarts_total);
        log_failure(&record, plan, self.inner.policy.max_restarts);
        if let Some(store) = self.inner.store.as_ref()
            && let Err(error) = store.persist(&record)
        {
            tracing::warn!(path = %store.path.display(), error = %format!("{error:#}"), "could not persist the render failure record");
        }
        // The failure, its counters, and the phase change together under
        // the failure lock, which `snapshot` holds while it reads them, so
        // no reader sees a counted restart without the failure behind it.
        let mut last_failure = self
            .inner
            .last_failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *last_failure = Some(record.clone());
        self.inner
            .consecutive_failures
            .store(consecutive_failures, Ordering::Relaxed);
        self.inner
            .restarts_total
            .store(restarts_total, Ordering::Relaxed);
        self.set_phase(phase);
        drop(last_failure);
        record
    }

    /// The record for a terminating failure that could not be logged or
    /// persisted, built without touching either.
    pub(crate) fn unrecorded_termination(&self, failure: RenderFailure) -> RenderFailureRecord {
        let restarts_total = self.inner.restarts_total.load(Ordering::Relaxed);
        failure_record(
            failure,
            RenderRecoveryAction::TerminateProcess,
            0,
            restarts_total,
        )
    }

    /// Ask the daemon to exit because the render thread cannot recover.
    pub(crate) fn request_termination(&self, record: Arc<RenderFailureRecord>) {
        self.set_phase(RenderThreadPhase::Terminating);
        self.inner.termination.send_replace(Some(record));
    }

    /// An observer for the process panic hook that persists render thread
    /// panics, or `None` when this supervision does not persist.
    ///
    /// In a `panic = "abort"` build this is the only code that runs between
    /// a render panic and the abort, so it persists a panic on the render
    /// thread or any of its runtime workers. An unwinding build persists
    /// only this supervision's own render thread: the supervisor records
    /// that unwind itself right after, while Tokio catches a worker panic
    /// and the supervisor only learns of it if the pipeline then fails.
    pub(crate) fn panic_observer(&self) -> Option<PanicObserver> {
        let store = self.inner.store.clone()?;
        let inner: Weak<SupervisionInner> = Arc::downgrade(&self.inner);
        Some(Arc::new(move |report: &PanicReport| {
            let Some(inner) = inner.upgrade() else {
                return;
            };
            let ours = if cfg!(panic = "abort") {
                is_render_thread_name(&report.thread_name)
            } else {
                inner
                    .render_thread
                    .try_lock()
                    .ok()
                    .and_then(|thread| *thread)
                    == Some(report.thread_id)
            };
            if !ours {
                return;
            }
            let action = if cfg!(panic = "abort") {
                RenderRecoveryAction::ProcessAbort
            } else {
                RenderRecoveryAction::AwaitingSupervisor
            };
            let failure = RenderFailure {
                kind: RenderFailureKind::Panic,
                message: report.payload.clone(),
                thread: Some(report.thread_name.clone()),
                location: report.location.clone(),
                backtrace: Some(report.backtrace_excerpt()),
            };
            let restarts_total = inner.restarts_total.load(Ordering::Relaxed);
            let record = failure_record(failure, action, 0, restarts_total);
            // Nothing useful can be done with an error from inside the
            // panic hook; the tracing record and stderr still carry it.
            let _ = store.persist(&record);
        }))
    }

    /// Queue a fault for the render thread to raise on itself.
    #[cfg(feature = "persistence-test-hooks")]
    #[doc(hidden)]
    pub fn inject_fault(&self, fault: InjectedRenderFault) {
        self.inner
            .injected_faults
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push_back(fault);
        self.inner
            .injected_fault_pending
            .store(true, Ordering::Release);
    }

    /// Report this render thread's compositor device lost, the way the
    /// device-lost callback raises a real device's loss signal.
    #[cfg(feature = "persistence-test-hooks")]
    #[doc(hidden)]
    pub fn inject_compositor_device_lost(&self, message: &str) {
        self.inner.injected_device_loss.raise(format!(
            "injected compositor device lost (unknown: {message})"
        ));
    }

    #[cfg(feature = "persistence-test-hooks")]
    pub(crate) fn take_injected_fault(&self) -> Option<InjectedRenderFault> {
        if !self.inner.injected_fault_pending.load(Ordering::Acquire) {
            return None;
        }
        let mut faults = self
            .inner
            .injected_faults
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let fault = faults.pop_front();
        if faults.is_empty() {
            self.inner
                .injected_fault_pending
                .store(false, Ordering::Release);
        }
        fault
    }

    /// Microseconds since this supervision's epoch, offset by one so zero
    /// can mean "never".
    fn stamp(&self, now: Instant) -> u64 {
        u64::try_from(now.saturating_duration_since(self.inner.epoch).as_micros())
            .unwrap_or(u64::MAX - 1)
            .saturating_add(1)
    }

    fn age(&self, stamp: &AtomicU64, now: Instant) -> Option<Duration> {
        let stamp = stamp.load(Ordering::Relaxed);
        (stamp != 0).then(|| Duration::from_micros(self.stamp(now).saturating_sub(stamp)))
    }
}

fn failure_record(
    failure: RenderFailure,
    action: RenderRecoveryAction,
    restart_attempt: u32,
    restarts_total: u64,
) -> RenderFailureRecord {
    let now = SystemTime::now();
    RenderFailureRecord {
        kind: failure.kind,
        message: failure.message,
        thread: failure.thread,
        location: failure.location,
        occurred_at: chrono::DateTime::<chrono::Utc>::from(now)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        occurred_at_unix_ms: now.duration_since(UNIX_EPOCH).map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        }),
        pid: std::process::id(),
        process_instance: PROCESS_INSTANCE.clone(),
        daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
        restart_attempt,
        restarts_total,
        action,
        backtrace: failure.backtrace,
    }
}

fn log_failure(record: &RenderFailureRecord, plan: RecoveryPlan, max_restarts: u32) {
    let backtrace = record.backtrace.as_deref().unwrap_or("unavailable");
    let thread = record.thread.as_deref().unwrap_or("unknown");
    let location = record.location.as_deref().unwrap_or("unknown");
    match plan {
        RecoveryPlan::Restart { attempt, backoff } => tracing::error!(
            kind = record.kind.as_str(),
            message = %record.message,
            thread,
            location,
            restart = attempt,
            max_restarts,
            backoff_ms = duration_millis(backoff),
            backtrace,
            "render pipeline failed; restarting it with fresh GPU and render state"
        ),
        RecoveryPlan::Terminate(TerminationCause::RestartBudgetExhausted { failures }) => {
            tracing::error!(
                kind = record.kind.as_str(),
                message = %record.message,
                thread,
                location,
                failures,
                max_restarts,
                backtrace,
                "render pipeline keeps failing; shutting the daemon down so its service manager restarts it"
            );
        }
        RecoveryPlan::Terminate(TerminationCause::SupervisorFailed) => tracing::error!(
            kind = record.kind.as_str(),
            message = %record.message,
            thread,
            location,
            backtrace,
            "render supervisor failed; shutting the daemon down so its service manager restarts it"
        ),
        RecoveryPlan::Terminate(TerminationCause::GpuDeviceLost) => tracing::error!(
            kind = record.kind.as_str(),
            message = %record.message,
            thread,
            location,
            backtrace,
            "compositor GPU device lost; shutting the daemon down so it reopens the GPU in a fresh process"
        ),
        RecoveryPlan::DaemonStopping => tracing::error!(
            kind = record.kind.as_str(),
            message = %record.message,
            thread,
            location,
            backtrace,
            "render pipeline failed while the daemon was stopping"
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{
        RENDER_FAILURE_FILE, RENDER_WEDGED_AFTER, RecoveryPlan, RenderFailure, RenderFailureKind,
        RenderFailureStore, RenderRecoveryAction, RenderRestartPolicy, RenderSupervision,
        RestartBudget, TerminationCause, failure_record,
    };

    #[test]
    fn a_record_from_this_process_is_not_reported_as_an_earlier_run() {
        let state_dir = tempfile::tempdir().expect("state tempdir should be created");
        let path = state_dir.path().join(RENDER_FAILURE_FILE);
        let store = RenderFailureStore { path: path.clone() };
        let record = failure_record(
            RenderFailure::new(RenderFailureKind::Panic, "this process".to_owned()),
            RenderRecoveryAction::Restart,
            1,
            1,
        );

        store.persist(&record).expect("record should persist");
        let supervision = RenderSupervision::new(RenderRestartPolicy::default(), Some(path));

        assert!(supervision.snapshot().last_failure.is_none());
        let leftovers: Vec<_> = std::fs::read_dir(state_dir.path())
            .expect("state dir should list")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(
            leftovers,
            vec![std::ffi::OsString::from(RENDER_FAILURE_FILE)]
        );
    }

    #[test]
    fn only_a_running_pipeline_that_stopped_iterating_fails_the_watchdog() {
        let supervision = RenderSupervision::detached();
        let start = Instant::now();
        let later = start + RENDER_WEDGED_AFTER + Duration::from_secs(1);
        assert!(supervision.watchdog_healthy_at(later), "not started yet");

        supervision.attempt_started(start);
        assert!(supervision.watchdog_healthy_at(start + Duration::from_secs(1)));
        assert!(!supervision.watchdog_healthy_at(later));

        supervision.heartbeat(later);
        assert!(supervision.watchdog_healthy_at(later + Duration::from_secs(1)));
        supervision.set_phase(super::RenderThreadPhase::Restarting);
        assert!(supervision.watchdog_healthy_at(later + RENDER_WEDGED_AFTER * 2));
    }

    fn policy() -> RenderRestartPolicy {
        RenderRestartPolicy {
            max_restarts: 3,
            window: Duration::from_secs(60),
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_millis(350),
            stable_uptime: Duration::from_secs(10),
        }
    }

    #[test]
    fn restarts_back_off_until_the_budget_runs_out() {
        let mut budget = RestartBudget::new(policy());
        let start = Instant::now();

        let plans: Vec<_> = (0..4)
            .map(|index| budget.record(start + Duration::from_secs(index), Duration::ZERO))
            .collect();

        assert_eq!(
            plans,
            vec![
                RecoveryPlan::Restart {
                    attempt: 1,
                    backoff: Duration::from_millis(100)
                },
                RecoveryPlan::Restart {
                    attempt: 2,
                    backoff: Duration::from_millis(200)
                },
                RecoveryPlan::Restart {
                    attempt: 3,
                    backoff: Duration::from_millis(350)
                },
                RecoveryPlan::Terminate(TerminationCause::RestartBudgetExhausted { failures: 4 }),
            ]
        );
    }

    #[test]
    fn failures_outside_the_window_leave_the_budget() {
        let mut budget = RestartBudget::new(policy());
        let start = Instant::now();
        for index in 0..3 {
            budget.record(start + Duration::from_secs(index), Duration::ZERO);
        }

        let plan = budget.record(start + Duration::from_secs(120), Duration::ZERO);

        assert!(matches!(plan, RecoveryPlan::Restart { attempt: 1, .. }));
    }

    #[test]
    fn a_stable_run_resets_the_backoff_streak() {
        let mut budget = RestartBudget::new(policy());
        let start = Instant::now();
        budget.record(start, Duration::ZERO);
        budget.record(start + Duration::from_secs(1), Duration::ZERO);

        let plan = budget.record(start + Duration::from_secs(20), Duration::from_secs(15));

        assert_eq!(
            plan,
            RecoveryPlan::Restart {
                attempt: 3,
                backoff: Duration::from_millis(100)
            }
        );
        assert_eq!(budget.consecutive(), 1);
    }
}
