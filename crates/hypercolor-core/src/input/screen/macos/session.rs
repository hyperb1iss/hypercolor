use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Instant;

use anyhow::anyhow;
#[cfg(target_os = "macos")]
use hypercolor_macos_capture::{
    MacosCaptureSelector, MacosDisplayClock, MacosScreenCaptureSession,
};
use hypercolor_macos_capture::{
    MacosProtectedSourceState as NativeProtectedSourceState, MacosScreenAuthorizationState,
    MacosScreenOwnerConflict, MacosScreenSelectionSnapshot, MacosScreenStatusSnapshot,
    MacosScreenTimingStatus,
};
use hypercolor_macos_input::MacosCapabilityOwner;

use crate::input::status::SourceSessionSlot;
use crate::input::{SourceKind, SourceStatusReporter};

use super::status::protected_screen_action_issue;
use super::{
    CaptureConfig, CaptureWorker, MacosCaptureControl, MacosExactPublicationShared,
    MacosPublication, MacosScreenCaptureInput, MacosScreenRuntimeTelemetry, PixelExtent,
    PreparedWorker, ScreenByteAdmissionCoordinator, ScreenCaptureDemand,
    ScreenComputeCapacityPolicy, StagedCaptureWorker, color_space_name, dynamic_range_name,
    executable_architecture, frame_drop_counters, lock, map_tahoe_capabilities,
    map_tahoe_selection_capabilities, nonzero_telemetry, pixel_format_name,
    production_stream_request, run_worker, timing_status, transfer_function_name,
};
#[cfg(feature = "macos-capture-fixtures")]
use super::{CapturePlanePool, InputSource, ScreenCaptureInput};
#[cfg(target_os = "macos")]
use super::{MacosSurfacePool, NativeCaptureControl};

impl MacosScreenCaptureInput {
    #[cfg(target_os = "macos")]
    pub fn new(
        config: CaptureConfig,
        admission: ScreenByteAdmissionCoordinator,
    ) -> anyhow::Result<Self> {
        Self::with_admission_and_compute_capacity(
            config,
            admission,
            ScreenComputeCapacityPolicy::UNBOUNDED,
        )
    }

    #[cfg(target_os = "macos")]
    fn with_admission_and_compute_capacity(
        config: CaptureConfig,
        admission: ScreenByteAdmissionCoordinator,
        compute_capacity_policy: ScreenComputeCapacityPolicy,
    ) -> anyhow::Result<Self> {
        let selector = MacosCaptureSelector::parse(&config.source)?;
        let host_capabilities = MacosScreenCaptureSession::capabilities()?;
        let request =
            production_stream_request(&config, ScreenCaptureDemand::Inactive, host_capabilities)?;
        let pool_coordinator = admission.clone();
        let telemetry = Arc::new(MacosScreenRuntimeTelemetry::renderer_authoritative());
        let pool_telemetry = Arc::clone(&telemetry);
        let session = MacosScreenCaptureSession::new_with_pool_admission(
            request,
            selector,
            move |conservative_surface_bytes, native_metadata_bytes| {
                let pool = MacosSurfacePool::reserve(
                    &pool_coordinator,
                    Arc::clone(&pool_telemetry),
                    conservative_surface_bytes,
                    native_metadata_bytes,
                )?;
                Ok(move |iosurface_id, allocation_bytes| {
                    let token = pool.observe(iosurface_id, allocation_bytes)?;
                    Ok(token as Arc<dyn Send + Sync>)
                })
            },
        )?;
        let clock = MacosDisplayClock::system()?;
        Ok(Self::with_control_and_telemetry(
            config,
            admission,
            compute_capacity_policy,
            Arc::new(NativeCaptureControl {
                session,
                clock,
                host_capabilities,
            }),
            telemetry,
            "screen_capture_kit_native",
        ))
    }

    #[cfg(any(target_os = "macos", feature = "macos-capture-fixtures"))]
    pub(super) fn with_control_and_telemetry(
        config: CaptureConfig,
        admission: ScreenByteAdmissionCoordinator,
        compute_capacity_policy: ScreenComputeCapacityPolicy,
        control: Arc<dyn MacosCaptureControl>,
        telemetry: Arc<MacosScreenRuntimeTelemetry>,
        backend: &'static str,
    ) -> Self {
        #[cfg(not(feature = "macos-capture-fixtures"))]
        let _ = (&admission, compute_capacity_policy);
        let consented = control.authorization() == MacosScreenAuthorizationState::Authorized;
        let authorization = control.authorization();
        let mut source = Self {
            config,
            control,
            #[cfg(feature = "macos-capture-fixtures")]
            admission,
            #[cfg(feature = "macos-capture-fixtures")]
            compute_capacity_policy,
            publication: Arc::new(Mutex::new(MacosPublication::default())),
            exact: Arc::new(MacosExactPublicationShared::with_compute_capacity_policy(
                compute_capacity_policy,
            )),
            telemetry,
            worker: None,
            worker_generation: 0,
            demand: ScreenCaptureDemand::Inactive,
            running: false,
            status: SourceStatusReporter::new(
                "macos:session",
                SourceKind::Screen,
                backend,
                true,
                consented,
                false,
            ),
            status_session: SourceSessionSlot::new(),
            owner: MacosCapabilityOwner::Standalone,
            owner_conflict: None,
            owner_designated_requirement_hash: None,
            authorization,
            authorization_last_transition_at: None,
            metal4: false,
        };
        source
            .refresh_platform_status()
            .expect("new macOS screen status is not retired");
        source
    }

    pub fn authorize(&mut self) -> anyhow::Result<NativeProtectedSourceState> {
        let state = self.control.request_authorization();
        self.refresh_policy()?;
        self.refresh_platform_status()?;
        Ok(state)
    }

    pub fn present_picker(&mut self) -> anyhow::Result<()> {
        let result = self.control.present_picker();
        self.refresh_platform_status()?;
        result
    }

    pub fn protected_state(&self) -> NativeProtectedSourceState {
        self.control.status()
    }

    pub fn set_capability_owner(&mut self, owner: MacosCapabilityOwner) -> anyhow::Result<()> {
        self.owner = owner;
        self.refresh_platform_status()
    }

    pub(super) fn refresh_platform_status(&mut self) -> anyhow::Result<()> {
        let state = self.control.status();
        let authorization = self.control.authorization();
        if authorization != self.authorization {
            self.authorization = authorization;
            self.authorization_last_transition_at = Some(Instant::now());
        }
        let diagnostics = self.control.diagnostics();
        let source = self.exact.source();
        let timing = MacosScreenTimingStatus {
            callback: timing_status(
                diagnostics.callback_sample_count,
                diagnostics.callback_total_ns,
                diagnostics.callback_max_ns,
                diagnostics.callback_p95_ns,
                diagnostics.callback_p99_ns,
            ),
            retain: timing_status(
                diagnostics.retain_sample_count,
                diagnostics.retain_total_ns,
                diagnostics.retain_max_ns,
                diagnostics.retain_p95_ns,
                diagnostics.retain_p99_ns,
            ),
            enqueue: timing_status(
                diagnostics.enqueue_sample_count,
                diagnostics.enqueue_total_ns,
                diagnostics.enqueue_max_ns,
                diagnostics.enqueue_p95_ns,
                diagnostics.enqueue_p99_ns,
            ),
            conversion: timing_status(
                diagnostics.conversion_sample_count,
                diagnostics.conversion_total_ns,
                diagnostics.conversion_max_ns,
                diagnostics.conversion_p95_ns,
                diagnostics.conversion_p99_ns,
            ),
            cpu_reduction: self.telemetry.cpu_reduction_timing.snapshot(),
            native_import: self.telemetry.native_import_timing.snapshot(),
            native_reduction_submit: self.telemetry.native_reduction_submit_timing.snapshot(),
            publication: timing_status(
                diagnostics.publication_sample_count,
                diagnostics.publication_total_ns,
                diagnostics.publication_max_ns,
                diagnostics.publication_p95_ns,
                diagnostics.publication_p99_ns,
            ),
            capture_to_native_publication: self
                .telemetry
                .capture_to_native_publication_timing
                .snapshot(),
            capture_to_converted_publication: self
                .telemetry
                .capture_to_converted_publication_timing
                .snapshot(),
        };
        let selection = self.control.selection();
        let host_capabilities = self.control.host_capabilities();
        let tahoe = map_tahoe_capabilities(host_capabilities, self.metal4);
        let tahoe_selection = self
            .control
            .tahoe_selection_capabilities()
            .map(map_tahoe_selection_capabilities);
        self.status
            .set_action_issue(protected_screen_action_issue(state))?;
        let status = MacosScreenStatusSnapshot {
            state,
            authorization,
            owner: Arc::from(self.owner.as_str()),
            selection: MacosScreenSelectionSnapshot {
                revision: self.control.selection_revision(),
                selection,
            },
            tahoe,
            tahoe_selection,
            owner_conflict: self
                .owner_conflict
                .as_ref()
                .map(|conflict| MacosScreenOwnerConflict {
                    active: Arc::from(conflict.active.as_str()),
                    contender: Arc::from(conflict.contender.as_str()),
                    observed_at_ms: conflict.observed_at_ms,
                }),
            authorization_last_transition_age_ms: self.authorization_last_transition_at.map(
                |transition| u64::try_from(transition.elapsed().as_millis()).unwrap_or(u64::MAX),
            ),
            owner_designated_requirement_hash: self.owner_designated_requirement_hash.clone(),
            executable_architecture: executable_architecture(),
            capture_session_generation: source
                .as_ref()
                .map(|source| source.epoch.session_generation),
            topology_generation: source
                .as_ref()
                .map(|source| source.epoch.topology_generation),
            resource_generation: source.as_ref().map(|source| source.resource_generation),
            publication_plan_generation: nonzero_telemetry(
                self.telemetry
                    .publication_plan_generation
                    .load(Ordering::Acquire),
            ),
            pixel_format: source
                .as_ref()
                .map(|source| Arc::from(pixel_format_name(source.pixel_format))),
            dynamic_range: source.as_ref().and_then(|source| {
                source
                    .colorimetry
                    .dynamic_range()
                    .map(|range| Arc::from(dynamic_range_name(range)))
            }),
            color_space: source
                .as_ref()
                .map(|source| Arc::from(color_space_name(source.colorimetry.color_space()))),
            transfer_function: source.as_ref().map(|source| {
                Arc::from(transfer_function_name(
                    source.colorimetry.transfer_function(),
                ))
            }),
            display_scale: source
                .as_ref()
                .map(|source| f64::from_bits(source.display_scale_bits)),
            native_width: source
                .as_ref()
                .map(|source| source.geometry.native_extent().width()),
            native_height: source
                .as_ref()
                .map(|source| source.geometry.native_extent().height()),
            queue_depth: hypercolor_macos_capture::MACOS_STREAM_QUEUE_DEPTH,
            admitted_native_bytes: self.telemetry.admitted_native_bytes.load(Ordering::Acquire),
            pinned_generations: self.telemetry.pinned_generations.load(Ordering::Acquire),
            frames_received: diagnostics.frames_received,
            frames_published: diagnostics.frames_published,
            frames_superseded: diagnostics.superseded_deliveries,
            frames_malformed: diagnostics.malformed_frames,
            frames_dropped: frame_drop_counters(&diagnostics).to_vec(),
            frames_stale: self.telemetry.stale_frames.load(Ordering::Acquire),
            publication_path: self.telemetry.publication_path(),
            fallback_reason: lock(&self.telemetry.fallback_reason).clone(),
            timing,
        };
        let diagnostics = hypercolor_macos_capture::screen_diagnostics_envelope(&status)
            .inspect_err(
                |error| tracing::warn!(%error, "dropping invalid macOS screen diagnostics"),
            )
            .ok();
        self.status.set_diagnostics(diagnostics)?;
        Ok(())
    }

    pub(super) fn refresh_policy(&mut self) -> anyhow::Result<()> {
        self.refresh_policy_for(self.demand)
    }

    pub(super) fn refresh_policy_for(&mut self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
        let consented = self.control.authorization() == MacosScreenAuthorizationState::Authorized;
        self.status
            .set_policy(true, consented, demand.is_active())?;
        Ok(())
    }

    #[cfg(not(feature = "macos-capture-fixtures"))]
    pub(super) fn prepare_worker(&self, _extent: PixelExtent) -> PreparedWorker {
        PreparedWorker {
            target_fps: self.config.target_fps,
        }
    }

    #[cfg(feature = "macos-capture-fixtures")]
    pub(super) fn prepare_worker(&self, extent: PixelExtent) -> anyhow::Result<PreparedWorker> {
        let mut analyzer = match self.compute_capacity_policy.analysis() {
            Some(capacity) => {
                ScreenCaptureInput::with_requested_extent_admission_and_compute_capacity(
                    self.config.clone(),
                    extent,
                    self.admission.clone(),
                    capacity,
                )?
            }
            None => ScreenCaptureInput::with_requested_extent_and_admission(
                self.config.clone(),
                extent,
                self.admission.clone(),
            )?,
        };
        analyzer.start()?;
        Ok(PreparedWorker {
            analyzer,
            plane_pool: CapturePlanePool::with_admission_coordinator(self.admission.clone()),
            target_fps: self.config.target_fps,
        })
    }

    pub(super) fn stage_worker(
        &self,
        prepared: PreparedWorker,
    ) -> anyhow::Result<StagedCaptureWorker> {
        let worker_generation = self
            .worker_generation
            .checked_add(1)
            .ok_or_else(|| anyhow!("macOS capture worker generation exhausted"))?;
        let mailbox = self.control.mailbox();
        let worker_mailbox = mailbox.clone();
        let control = Arc::clone(&self.control);
        let publication = Arc::clone(&self.publication);
        let exact = Arc::clone(&self.exact);
        let telemetry = Arc::clone(&self.telemetry);
        let status_session = self.status_session.clone();
        let target_fps = prepared.target_fps;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let start = Arc::new(AtomicBool::new(false));
        let worker_start = Arc::clone(&start);
        let (exit_tx, exit_rx) = mpsc::channel();
        let (command_tx, command_rx) = mpsc::channel();
        let join = thread::Builder::new()
            .name("hypercolor-macos-screen-capture".to_owned())
            .spawn(move || {
                while !worker_start.load(Ordering::Acquire) {
                    thread::park();
                }
                let result = if worker_stop.load(Ordering::Acquire) {
                    Ok(())
                } else {
                    run_worker(
                        prepared,
                        mailbox,
                        publication,
                        exact,
                        telemetry,
                        worker_generation,
                        target_fps,
                        status_session,
                        worker_stop,
                        control,
                        command_rx,
                    )
                };
                // The exit channel is only drained by the legacy sampling
                // path; the exact-publication path never observes it, so an
                // unlogged error here is a silently dead capture pump.
                if let Err(error) = &result {
                    tracing::warn!(
                        error = format!("{error:#}"),
                        "macOS screen capture worker exited with error"
                    );
                } else {
                    tracing::debug!("macOS screen capture worker exited cleanly");
                }
                let _ = exit_tx.send(result);
            })?;
        Ok(StagedCaptureWorker {
            generation: worker_generation,
            worker: Some(CaptureWorker {
                stop,
                mailbox: worker_mailbox,
                command_tx,
                exit_rx,
                join: Some(join),
            }),
            start,
        })
    }

    pub(super) fn install_worker(&mut self, staged: StagedCaptureWorker) {
        let generation = staged.generation;
        let start = Arc::clone(&staged.start);
        let worker = staged.commit();
        #[cfg(feature = "macos-capture-fixtures")]
        let previous_latest = lock(&self.publication).latest.clone();
        self.stop_worker();
        self.worker_generation = generation;
        {
            let mut publication = lock(&self.publication);
            publication.worker_generation = generation;
            #[cfg(feature = "macos-capture-fixtures")]
            {
                publication.latest = previous_latest;
            }
        }
        self.worker = Some(worker);
        start.store(true, Ordering::Release);
        self.worker
            .as_ref()
            .and_then(|worker| worker.join.as_ref())
            .expect("installed worker retains its thread handle")
            .thread()
            .unpark();
    }

    pub(super) fn stop_worker(&mut self) {
        let Some(mut worker) = self.worker.take() else {
            return;
        };
        worker.stop.store(true, Ordering::Release);
        worker.mailbox.wake();
        if let Some(join) = worker.join.take() {
            let _ = join.join();
        }
        #[cfg(feature = "macos-capture-fixtures")]
        {
            lock(&self.publication).latest = None;
        }
        self.exact.replace_source(None);
    }

    pub(super) fn observe_worker_exit(&mut self) -> anyhow::Result<()> {
        let Some(worker) = self.worker.as_ref() else {
            return Ok(());
        };
        match worker.exit_rx.try_recv() {
            Ok(Ok(())) => {
                self.stop_worker();
                if self.running && self.demand.is_active() {
                    return Err(anyhow!("macOS capture worker exited while active"));
                }
            }
            Ok(Err(error)) => {
                self.stop_worker();
                return Err(error);
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.stop_worker();
                return Err(anyhow!("macOS capture worker disconnected"));
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        Ok(())
    }
}
