use super::admission::prepare_macos_exact_runtime;
use super::publication::{capture_source_id, publish_frame};
use super::{
    Arc, AtomicBool, CaptureWorker, MacosCaptureControl, MacosExactPublicationShared,
    MacosExactRuntime, MacosFrameEvent, MacosFrameMailbox, MacosFrameStatus, MacosPublication,
    MacosScreenRuntimeTelemetry, Mutex, Ordering, PreparedWorker, ResourceState,
    ScreenPublicationHealth, ScreenPublicationHub, ScreenPublicationHubError, ScreenWorkerBinding,
    SourceSessionSlot, StagedCaptureWorker, TopologyState, WORKER_WAIT, WorkerCommand, anyhow,
    mpsc,
};
#[cfg(feature = "macos-capture-fixtures")]
use super::{InputSource, lock};

impl StagedCaptureWorker {
    pub(super) fn commit(mut self) -> CaptureWorker {
        self.worker
            .take()
            .expect("staged capture worker commits exactly once")
    }
}

impl Drop for StagedCaptureWorker {
    fn drop(&mut self) {
        let Some(mut worker) = self.worker.take() else {
            return;
        };
        worker.stop.store(true, Ordering::Release);
        self.start.store(true, Ordering::Release);
        if let Some(join) = worker.join.take() {
            join.thread().unpark();
            let _ = join.join();
        }
    }
}

pub(super) fn reap_macos_exact_runtimes(
    runtimes: &mut Vec<MacosExactRuntime>,
    exact: &MacosExactPublicationShared,
) {
    exact.reap_owned_sources();
    let authority = exact.hub().map(|hub| hub.committed_state());
    runtimes.retain(|runtime| {
        authority
            .as_ref()
            .is_some_and(|authority| authority.owns_runtime_binding(&runtime.binding))
    });
}

pub(super) fn handle_worker_commands(
    command_rx: &mpsc::Receiver<WorkerCommand>,
    prepared: &mut PreparedWorker,
    runtimes: &mut Vec<MacosExactRuntime>,
    exact: &MacosExactPublicationShared,
) {
    #[cfg(not(feature = "macos-capture-fixtures"))]
    let _ = prepared;
    while let Ok(command) = command_rx.try_recv() {
        match command {
            WorkerCommand::PrepareExact {
                ticket,
                cancelled,
                completion,
            } => {
                if cancelled.load(Ordering::Acquire) {
                    let _ = completion.send(Err(anyhow!(
                        "macOS exact publication preparation was cancelled"
                    )));
                    continue;
                }
                let source = exact.source();
                match prepare_macos_exact_runtime(ticket, source.as_ref(), exact) {
                    Ok((token, runtime)) if !cancelled.load(Ordering::Acquire) => {
                        if let Some((runtime, owned_source)) = runtime {
                            exact.register_owned_source(owned_source);
                            runtimes.push(runtime);
                        }
                        if completion.send(Ok(token)).is_err() {
                            reap_macos_exact_runtimes(runtimes, exact);
                        }
                    }
                    Ok((_token, _runtime)) => {
                        let _ = completion.send(Err(anyhow!(
                            "macOS exact publication preparation was cancelled"
                        )));
                    }
                    Err(error) => {
                        let _ = completion.send(Err(error));
                    }
                }
            }
            WorkerCommand::ReapExact { completion } => {
                reap_macos_exact_runtimes(runtimes, exact);
                if let Some(completion) = completion {
                    let _ = completion.send(Ok(()));
                }
            }
            #[cfg(feature = "macos-capture-fixtures")]
            WorkerCommand::ReconfigureProcessing {
                calibration,
                completion,
            } => {
                prepared.analyzer.set_led_tone_map_calibration(calibration);
                let _ = completion.send(Ok(()));
            }
        }
    }
}

pub(super) fn update_pinned_generations(
    runtimes: &[MacosExactRuntime],
    telemetry: &MacosScreenRuntimeTelemetry,
) {
    let current = runtimes
        .iter()
        .map(|runtime| runtime.source.resource_generation)
        .max();
    let mut retained = runtimes
        .iter()
        .filter(|runtime| Some(runtime.source.resource_generation) != current)
        .map(|runtime| runtime.source.resource_generation)
        .collect::<Vec<_>>();
    retained.sort_unstable();
    retained.dedup();
    telemetry
        .pinned_generations
        .store(retained.len(), Ordering::Release);
}

pub(super) fn with_current_macos_worker_authority<T>(
    exact: &MacosExactPublicationShared,
    runtimes: &[MacosExactRuntime],
    operation: impl FnOnce(
        &ScreenPublicationHub,
        &ScreenWorkerBinding,
    ) -> Result<T, ScreenPublicationHubError>,
) -> anyhow::Result<Option<T>> {
    let Some(hub) = exact.hub() else {
        return Ok(None);
    };
    let authority = hub.committed_state();
    let Some(binding) = runtimes
        .iter()
        .map(|runtime| &runtime.binding)
        .find(|binding| authority.owns_runtime_binding(binding))
    else {
        return Ok(None);
    };
    match operation(&hub, binding) {
        Ok(value) => Ok(Some(value)),
        Err(ScreenPublicationHubError::WorkerAuthorityStale { .. }) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn report_macos_worker_health(
    exact: &MacosExactPublicationShared,
    runtimes: &[MacosExactRuntime],
    health: ScreenPublicationHealth,
) -> anyhow::Result<()> {
    with_current_macos_worker_authority(exact, runtimes, |hub, binding| {
        hub.report_worker_delivery_health(binding, health)
    })?;
    Ok(())
}

pub(super) fn invalidate_macos_worker(
    exact: &MacosExactPublicationShared,
    runtimes: &[MacosExactRuntime],
) -> anyhow::Result<()> {
    with_current_macos_worker_authority(exact, runtimes, ScreenPublicationHub::invalidate_worker)?;
    Ok(())
}

pub(super) fn synchronize_macos_invalidation_generation(
    observed: &mut u64,
    delivered: u64,
    publication: &Arc<Mutex<MacosPublication>>,
    exact: &MacosExactPublicationShared,
    runtimes: &[MacosExactRuntime],
) -> anyhow::Result<bool> {
    #[cfg(not(feature = "macos-capture-fixtures"))]
    let _ = publication;
    if delivered < *observed {
        return Ok(false);
    }
    if delivered > *observed {
        #[cfg(feature = "macos-capture-fixtures")]
        {
            lock(publication).latest = None;
        }
        invalidate_macos_worker(exact, runtimes)?;
        *observed = delivered;
    }
    Ok(true)
}

pub(super) fn run_worker(
    mut prepared: PreparedWorker,
    mailbox: MacosFrameMailbox,
    publication: Arc<Mutex<MacosPublication>>,
    exact: Arc<MacosExactPublicationShared>,
    telemetry: Arc<MacosScreenRuntimeTelemetry>,
    worker_generation: u64,
    target_fps: u32,
    status_session: SourceSessionSlot,
    stop: Arc<AtomicBool>,
    control: Arc<dyn MacosCaptureControl>,
    command_rx: mpsc::Receiver<WorkerCommand>,
) -> anyhow::Result<()> {
    let mut topology = TopologyState::default();
    let mut resources = ResourceState::default();
    let mut exact_runtimes = Vec::new();
    let mut invalidation_generation = 0;
    let result: anyhow::Result<()> = (|| {
        while !stop.load(Ordering::Acquire) {
            handle_worker_commands(&command_rx, &mut prepared, &mut exact_runtimes, &exact);
            update_pinned_generations(&exact_runtimes, &telemetry);
            let Some((_, delivery_invalidation_generation, delivery)) = mailbox
                .wait_latest_with_generation_while(WORKER_WAIT, || !stop.load(Ordering::Acquire))
            else {
                continue;
            };
            if !synchronize_macos_invalidation_generation(
                &mut invalidation_generation,
                delivery_invalidation_generation,
                &publication,
                &exact,
                &exact_runtimes,
            )? {
                continue;
            }
            match delivery {
                Ok(MacosFrameEvent::Frame(frame)) => {
                    publish_frame(
                        &mut prepared,
                        Arc::from(frame),
                        capture_source_id(control.selection())?,
                        &mut topology,
                        &mut resources,
                        &publication,
                        &exact,
                        &telemetry,
                        &mut exact_runtimes,
                        worker_generation,
                        target_fps,
                        &status_session,
                        &control,
                    )?;
                }
                Ok(MacosFrameEvent::Lifecycle(
                    MacosFrameStatus::Suspended | MacosFrameStatus::Stopped,
                ))
                | Err(_) => {}
                Ok(MacosFrameEvent::RecoverableError(_)) => {
                    report_macos_worker_health(
                        &exact,
                        &exact_runtimes,
                        ScreenPublicationHealth::Recovering,
                    )?;
                }
                Ok(MacosFrameEvent::Lifecycle(_)) => {}
            }
        }
        Ok(())
    })();
    let invalidation = invalidate_macos_worker(&exact, &exact_runtimes);
    exact.replace_source(None);
    exact.clear_owned_sources();
    exact_runtimes.clear();
    telemetry.pinned_generations.store(0, Ordering::Release);
    #[cfg(feature = "macos-capture-fixtures")]
    prepared.analyzer.stop();
    result?;
    invalidation
}
