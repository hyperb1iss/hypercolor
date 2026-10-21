//! Body of the `hypercolor-render` thread: start the render runtime and the
//! input publication pump, build the pipeline, and keep a pipeline running.
//!
//! A pipeline that panics, loses its input pump, or loses its compositor
//! GPU device ends its attempt. The supervisor records the failure, and
//! either rebuilds the pipeline in-process with fresh GPU and render state
//! (within the restart budget, after a backoff) or asks the daemon to exit
//! so its service manager starts a fresh process. A panic anywhere else on
//! this thread, in startup or in the supervisor itself, is caught at the
//! top of the thread and also ends in that exit, so the thread never ends
//! without the daemon knowing.
//!
//! What a restart rebuilds: both compositors and every GPU resource on
//! them, the zone runtime with its effect renderers and Servo sessions, the
//! scene snapshot and frame state, and the input pump when it is the part
//! that failed. What it keeps: the compositor GPU device, which stays valid
//! after a panic or a validation error, and the shared daemon state the
//! pipeline reads. A lost device is not rebuilt in-process: Servo's GPU
//! import device is a process-wide once-only binding to the device opened
//! at startup, and the native screen bridges and the interactive preview
//! device came from the same adapter, so only a fresh process can rebind
//! all of them.

use std::panic::AssertUnwindSafe;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use arc_swap::ArcSwapOption;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use super::gpu_errors::DeviceLossSignal;
use super::input_publication::{InputPublicationMonitor, InputPublicationPump};
use super::pipeline_driver::run_pipeline;
use super::pipeline_runtime::PipelineRuntime;
#[cfg(feature = "wgpu")]
use super::screen_parity_diagnostics::ScreenParityDiagnosticMailbox;
use super::supervision::{
    RENDER_RUNTIME_THREAD_NAME, RecoveryPlan, RenderFailure, RenderFailureKind, RenderSupervision,
    RenderThreadPhase, RestartBudget, TerminationCause,
};
use super::{InputPublicationDemandHandle, RenderThreadState, panic_payload_message};
use crate::startup::{StartupProgress, startup_step};

/// How recent a runtime worker panic must be to explain a pump failure.
const RECENT_WORKER_PANIC: Duration = Duration::from_secs(10);

/// Everything the render thread takes ownership of when it starts.
pub(super) struct RenderThreadLaunch<F> {
    pub(super) state: RenderThreadState,
    pub(super) build_runtime: F,
    pub(super) progress: Option<StartupProgress>,
    pub(super) ready: mpsc::SyncSender<Result<()>>,
    pub(super) cancel: CancellationToken,
    pub(super) input_demands: InputPublicationDemandHandle,
    pub(super) input_monitor: Arc<ArcSwapOption<InputPublicationMonitor>>,
    pub(super) supervision: RenderSupervision,
    #[cfg(feature = "wgpu")]
    pub(super) screen_parity_mailbox: ScreenParityDiagnosticMailbox,
}

/// Run the render thread until shutdown, a clean loop exit, or a failure
/// the supervisor gives up on.
///
/// Startup failures are reported through `ready` and end the thread with
/// `Ok`, so the spawner sees the error instead of the join result. A panic
/// that escapes everything below is recorded and requests termination.
pub(super) fn run<F>(launch: RenderThreadLaunch<F>) -> Result<()>
where
    F: FnOnce() -> Result<tokio::runtime::Runtime>,
{
    let supervision = launch.supervision.clone();
    supervision.bind_render_thread();
    std::panic::catch_unwind(AssertUnwindSafe(|| run_supervised(launch))).unwrap_or_else(
        |payload| {
            let failure = RenderFailure::panic(
                panic_payload_message(payload.as_ref()),
                crate::panic_hook::take_recent_report(std::thread::current().id()),
            );
            // Recording logs and writes a file; if that panics too, the
            // termination request below must still go out.
            let record = std::panic::catch_unwind(AssertUnwindSafe(|| {
                supervision.record_failure(
                    failure.clone(),
                    RecoveryPlan::Terminate(TerminationCause::SupervisorFailed),
                    0,
                )
            }))
            .unwrap_or_else(|_| supervision.unrecorded_termination(failure));
            let error = anyhow!(
                "render thread panicked outside a pipeline attempt: {}",
                record.message
            );
            supervision.request_termination(Arc::new(record));
            Err(error)
        },
    )
}

fn run_supervised<F>(launch: RenderThreadLaunch<F>) -> Result<()>
where
    F: FnOnce() -> Result<tokio::runtime::Runtime>,
{
    let RenderThreadLaunch {
        state,
        build_runtime,
        progress,
        ready,
        cancel,
        input_demands,
        input_monitor,
        supervision,
        #[cfg(feature = "wgpu")]
        screen_parity_mailbox,
    } = launch;
    let _scene_transaction_consumer = state.scene_transactions.consumer();
    crate::process::configure_render_thread_priority();
    let runtime = match startup_step(progress.as_ref(), "render runtime", build_runtime) {
        Ok(runtime) => runtime,
        Err(error) => {
            supervision.set_phase(RenderThreadPhase::Stopped);
            let _ = ready.send(Err(error));
            return Ok(());
        }
    };
    let mut input_pump = match startup_step(progress.as_ref(), "input publication pump", || {
        runtime.block_on(start_input_pump(&state, input_demands.clone()))
    }) {
        Ok(pump) => pump,
        Err(error) => {
            supervision.set_phase(RenderThreadPhase::Stopped);
            let _ = ready.send(Err(error));
            return Ok(());
        }
    };
    let pipeline = build_pipeline(
        &state,
        &input_pump,
        &input_demands,
        #[cfg(feature = "wgpu")]
        &screen_parity_mailbox,
        progress.as_ref(),
    );
    // Startup reporting ends with the pipeline build.
    drop(progress);
    let pipeline = match pipeline {
        Ok(pipeline) => pipeline,
        Err(error) => {
            if let Err(shutdown_error) = runtime.block_on(input_pump.shutdown()) {
                warn!(%shutdown_error, "input publication cleanup failed after render startup error");
            }
            supervision.set_phase(RenderThreadPhase::Stopped);
            let _ = ready.send(Err(error));
            return Ok(());
        }
    };
    input_monitor.store(Some(Arc::new(input_pump.monitor())));
    if ready.send(Ok(())).is_err() {
        supervision.set_phase(RenderThreadPhase::Stopped);
        return runtime.block_on(input_pump.shutdown());
    }

    let mut supervisor = Supervisor {
        runtime: &runtime,
        state: &state,
        cancel: &cancel,
        input_demands: &input_demands,
        input_monitor: &input_monitor,
        budget: RestartBudget::new(supervision.policy()),
        supervision: &supervision,
        device_loss: None,
        input_pump,
        #[cfg(feature = "wgpu")]
        screen_parity_mailbox: &screen_parity_mailbox,
    };
    let result = supervisor.supervise(pipeline);
    let shutdown_result = runtime.block_on(supervisor.input_pump.shutdown());
    result.and(shutdown_result)
}

/// How one pipeline attempt ended.
enum AttemptOutcome {
    /// The render loop is no longer running or paused.
    Exited,
    /// The render thread was asked to stop.
    Cancelled,
    Failed(RenderFailure),
}

/// What the supervisor does next after a failure.
enum Flow {
    RetryAfter(Duration),
    Stop(Result<()>),
}

struct Supervisor<'a> {
    runtime: &'a tokio::runtime::Runtime,
    state: &'a RenderThreadState,
    cancel: &'a CancellationToken,
    input_demands: &'a InputPublicationDemandHandle,
    input_monitor: &'a Arc<ArcSwapOption<InputPublicationMonitor>>,
    supervision: &'a RenderSupervision,
    budget: RestartBudget,
    /// Loss signal of the GPU device the pipelines draw with. Every rebuild
    /// keeps the same device, so a loss stays visible to later attempts.
    device_loss: Option<DeviceLossSignal>,
    input_pump: InputPublicationPump,
    #[cfg(feature = "wgpu")]
    screen_parity_mailbox: &'a ScreenParityDiagnosticMailbox,
}

impl Supervisor<'_> {
    fn supervise(&mut self, first: PipelineRuntime) -> Result<()> {
        let mut next = Some(first);
        let mut restarted = false;
        loop {
            let built = match next.take() {
                Some(pipeline) => Ok(pipeline),
                None => self.rebuild(),
            };
            let (failure, uptime) = match built {
                Ok(pipeline) => {
                    self.device_loss = pipeline.render.sparkleflinger.device_loss_signal();
                    if restarted {
                        info!(
                            restarts_total = self.supervision.snapshot().restarts_total,
                            "render pipeline restarted with fresh GPU and render state"
                        );
                    }
                    let started = Instant::now();
                    self.supervision.attempt_started(started);
                    match self.run_attempt(pipeline) {
                        AttemptOutcome::Exited => {
                            self.supervision.set_phase(RenderThreadPhase::Stopped);
                            return Ok(());
                        }
                        AttemptOutcome::Cancelled => {
                            self.runtime
                                .block_on(async { self.state.render_loop.write().await.stop() });
                            self.supervision.set_phase(RenderThreadPhase::Stopped);
                            return Ok(());
                        }
                        AttemptOutcome::Failed(failure) => (failure, started.elapsed()),
                    }
                }
                Err(failure) => (failure, Duration::ZERO),
            };
            match self.handle_failure(failure, uptime) {
                Flow::RetryAfter(backoff) => {
                    if !self.wait_backoff(backoff) {
                        return Ok(());
                    }
                    restarted = true;
                }
                Flow::Stop(result) => return result,
            }
        }
    }

    /// Run one pipeline until it exits, fails, or the thread is cancelled.
    ///
    /// The pipeline future runs on this thread inside `block_on`, so a
    /// panic anywhere in a frame unwinds to the `catch_unwind` here. The
    /// unwind drops the pipeline and everything it owned. Shared daemon
    /// state behind Tokio locks survives intact, since those do not
    /// poison. A std lock the panic poisoned can make the rebuilt pipeline
    /// (or its rebuild) fail again, because some of its users refuse a
    /// poisoned lock; the restart budget then runs out and ends in a
    /// process exit, which is the recovery such a failure needs.
    fn run_attempt(&self, pipeline: PipelineRuntime) -> AttemptOutcome {
        let monitor = self.input_pump.monitor();
        let state = self.state.clone();
        let supervision = self.supervision.clone();
        let cancel = self.cancel.clone();
        let device_loss = self.device_loss_wait();
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
            self.runtime.block_on(async move {
                tokio::select! {
                    () = run_pipeline(state, pipeline, supervision) => AttemptOutcome::Exited,
                    status = monitor.wait_for_terminal() => AttemptOutcome::Failed(
                        RenderFailure::new(
                            RenderFailureKind::InputPumpTerminated,
                            format!("input publication pump terminated while rendering: {status:?}"),
                        )
                        .with_panic_report(crate::panic_hook::take_recent_report_where(|report| {
                            report.thread_name.starts_with(RENDER_RUNTIME_THREAD_NAME)
                                && report
                                    .occurred_at
                                    .elapsed()
                                    .is_ok_and(|age| age < RECENT_WORKER_PANIC)
                        })),
                    ),
                    message = device_loss => AttemptOutcome::Failed(
                        RenderFailure::new(RenderFailureKind::GpuDeviceLost, message),
                    ),
                    () = cancel.cancelled() => AttemptOutcome::Cancelled,
                }
            })
        }));
        outcome.unwrap_or_else(|payload| {
            let message = panic_payload_message(payload.as_ref());
            let report = crate::panic_hook::take_recent_report(std::thread::current().id());
            AttemptOutcome::Failed(RenderFailure::panic(message, report))
        })
    }

    /// The compositor device's loss description, once it is lost.
    fn device_lost(&self) -> Option<String> {
        let lost = self
            .device_loss
            .as_ref()
            .and_then(DeviceLossSignal::lost)
            .map(|lost| lost.to_string());
        #[cfg(feature = "persistence-test-hooks")]
        let lost = lost.or_else(|| {
            self.supervision
                .injected_device_loss()
                .lost()
                .map(|lost| lost.to_string())
        });
        lost
    }

    /// Resolves when the compositor device is lost, immediately when it
    /// already was.
    fn device_loss_wait(&self) -> impl Future<Output = String> + use<> {
        let device = self.device_loss.clone();
        #[cfg(feature = "persistence-test-hooks")]
        let injected = self.supervision.injected_device_loss();
        async move {
            let device = async move {
                match device {
                    Some(device) => device.wait().await,
                    None => std::future::pending().await,
                }
            };
            #[cfg(feature = "persistence-test-hooks")]
            let device = async move {
                tokio::select! {
                    message = device => message,
                    message = injected.wait() => message,
                }
            };
            device.await
        }
    }

    fn handle_failure(&mut self, failure: RenderFailure, uptime: Duration) -> Flow {
        let failure = if failure.kind != RenderFailureKind::GpuDeviceLost
            && let Some(lost) = self.device_lost()
        {
            failure.after_device_loss(&lost)
        } else {
            failure
        };
        let plan = if self.cancel.is_cancelled() {
            RecoveryPlan::DaemonStopping
        } else if failure.kind == RenderFailureKind::GpuDeviceLost {
            RecoveryPlan::Terminate(TerminationCause::GpuDeviceLost)
        } else {
            self.budget.record(Instant::now(), uptime)
        };
        let record = self
            .supervision
            .record_failure(failure, plan, self.budget.consecutive());
        match plan {
            RecoveryPlan::Restart { backoff, .. } => Flow::RetryAfter(backoff),
            RecoveryPlan::DaemonStopping => Flow::Stop(Ok(())),
            RecoveryPlan::Terminate(_) => {
                let error = anyhow!(
                    "render pipeline cannot recover ({}): {}",
                    record.kind.as_str(),
                    record.message
                );
                self.supervision.request_termination(Arc::new(record));
                Flow::Stop(Err(error))
            }
        }
    }

    /// Sleep out a restart backoff. Returns `false` when the thread was
    /// cancelled meanwhile.
    fn wait_backoff(&self, backoff: Duration) -> bool {
        let cancel = self.cancel.clone();
        let restart = self.runtime.block_on(async move {
            tokio::select! {
                () = tokio::time::sleep(backoff) => true,
                () = cancel.cancelled() => false,
            }
        });
        if !restart {
            self.supervision.set_phase(RenderThreadPhase::Stopped);
        }
        restart
    }

    /// Build a replacement pipeline, restarting the input pump first when
    /// it was the part that failed. A panic during the rebuild is a failure
    /// like any other and counts against the restart budget.
    fn rebuild(&mut self) -> Result<PipelineRuntime, RenderFailure> {
        std::panic::catch_unwind(AssertUnwindSafe(|| self.try_rebuild())).unwrap_or_else(
            |payload| {
                let message = panic_payload_message(payload.as_ref());
                let report = crate::panic_hook::take_recent_report(std::thread::current().id());
                let mut failure = RenderFailure::panic(message, report);
                failure.message = format!("while rebuilding the pipeline: {}", failure.message);
                Err(failure)
            },
        )
    }

    fn try_rebuild(&mut self) -> Result<PipelineRuntime, RenderFailure> {
        if self.input_pump.monitor().status().is_terminal() {
            self.restart_input_pump()?;
        }
        build_pipeline(
            self.state,
            &self.input_pump,
            self.input_demands,
            #[cfg(feature = "wgpu")]
            self.screen_parity_mailbox,
            None,
        )
        .map_err(|error| {
            RenderFailure::from_error(RenderFailureKind::PipelineRebuildFailed, &error)
        })
    }

    fn restart_input_pump(&mut self) -> Result<(), RenderFailure> {
        if let Err(error) = self.runtime.block_on(self.input_pump.shutdown()) {
            debug!(%error, "failed input publication pump reported its failure again during cleanup");
        }
        let pump = self
            .runtime
            .block_on(start_input_pump(self.state, self.input_demands.clone()))
            .map_err(|error| {
                RenderFailure::from_error(RenderFailureKind::InputPumpRestartFailed, &error)
            })?;
        self.input_pump = pump;
        self.input_monitor
            .store(Some(Arc::new(self.input_pump.monitor())));
        info!("input publication pump restarted");
        Ok(())
    }
}

async fn start_input_pump(
    state: &RenderThreadState,
    demands: InputPublicationDemandHandle,
) -> Result<InputPublicationPump> {
    InputPublicationPump::start(
        state.input_manager.clone(),
        demands,
        Some(state.interaction_routing.device_input().clone()),
    )
    .await
}

fn build_pipeline(
    state: &RenderThreadState,
    input_pump: &InputPublicationPump,
    input_demands: &InputPublicationDemandHandle,
    #[cfg(feature = "wgpu")] screen_parity_mailbox: &ScreenParityDiagnosticMailbox,
    progress: Option<&StartupProgress>,
) -> Result<PipelineRuntime> {
    PipelineRuntime::from_state(
        state,
        input_pump.reader(),
        input_demands.clone(),
        #[cfg(feature = "wgpu")]
        screen_parity_mailbox.clone(),
        progress,
    )
}
