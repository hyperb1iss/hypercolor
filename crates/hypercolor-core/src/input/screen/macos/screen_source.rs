#[cfg(feature = "macos-capture-fixtures")]
use std::sync::mpsc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::anyhow;
use hypercolor_macos_capture::MacosScreenAuthorizationState;
use hypercolor_macos_input::{MacosCapabilityOwner, MacosDaemonOwnerConflict};
use tokio::sync::oneshot;

use super::{
    CaptureConfig, CaptureSourceId, LedToneMapCalibration, MacosScreenCaptureInput, PreparedWorker,
    ProtectedSourceAuthorizationAction, RegisteredScreenBranchDemand, ResolvedScreenBranchDemand,
    ScreenAnalysisComputeCapacity, ScreenAnalysisResourcePlan, ScreenAnalysisWorkPlan,
    ScreenCaptureDemand, ScreenPublicationHub, ScreenPublicationRequest,
    ScreenRendererExecutionState, ScreenSource, ScreenSourcePickerAction, ScreenWorkerPreparation,
    ScreenWorkerPreparationTicket, ScreenWorkerRetirement, SourceCapabilityContext,
    SourceDiagnosticArtifactAction, WorkerCommand, production_stream_request,
    protected_action_identity, resolve_macos_publication_branch_with_telemetry,
};
#[cfg(feature = "macos-capture-fixtures")]
use super::{CapturePlanePool, InputSource, ScreenCaptureInput};

impl ScreenSource for MacosScreenCaptureInput {
    fn set_capability_context(&mut self, context: &SourceCapabilityContext) -> anyhow::Result<()> {
        let Some(owner) = MacosCapabilityOwner::from_id(&context.owner) else {
            return Ok(());
        };
        let conflict = context.conflict.as_ref().and_then(|conflict| {
            Some(MacosDaemonOwnerConflict {
                active: MacosCapabilityOwner::from_id(&conflict.active)?,
                contender: MacosCapabilityOwner::from_id(&conflict.contender)?,
                observed_at_ms: conflict.observed_at_ms,
            })
        });
        self.owner = owner;
        self.owner_conflict = conflict.map(Arc::new);
        self.owner_designated_requirement_hash
            .clone_from(&context.identity_hash);
        self.metal4 = context.features.get("metal4").copied().unwrap_or(false);
        self.refresh_platform_status()
    }
    fn screen_capture_demand(&self) -> ScreenCaptureDemand {
        self.demand
    }

    fn screen_analysis_resource_plan(
        &self,
        demand: ScreenCaptureDemand,
    ) -> anyhow::Result<Option<ScreenAnalysisResourcePlan>> {
        #[cfg(feature = "macos-capture-fixtures")]
        {
            let Some(extent) = demand.requested_extent() else {
                return Ok(None);
            };
            Ok(Some(ScreenAnalysisResourcePlan::try_new_for_extent(
                self.config.grid_cols,
                self.config.grid_rows,
                self.config.target_fps,
                extent,
                u64::MAX,
            )?))
        }
        #[cfg(not(feature = "macos-capture-fixtures"))]
        {
            let _ = demand;
            Ok(None)
        }
    }

    fn screen_analysis_work_plan(
        &self,
        demand: ScreenCaptureDemand,
    ) -> anyhow::Result<Option<ScreenAnalysisWorkPlan>> {
        #[cfg(feature = "macos-capture-fixtures")]
        {
            let Some(extent) = demand.requested_extent() else {
                return Ok(None);
            };
            Ok(Some(ScreenAnalysisWorkPlan::try_new(
                extent,
                extent,
                &self.config,
            )?))
        }
        #[cfg(not(feature = "macos-capture-fixtures"))]
        {
            let _ = demand;
            Ok(None)
        }
    }

    fn screen_analysis_compute_capacity(&self) -> Option<ScreenAnalysisComputeCapacity> {
        #[cfg(feature = "macos-capture-fixtures")]
        {
            self.compute_capacity_policy.analysis()
        }
        #[cfg(not(feature = "macos-capture-fixtures"))]
        {
            None
        }
    }

    fn set_screen_capture_demand(&mut self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
        let was_active = self.demand.is_active();
        if !demand.is_active() {
            self.refresh_policy_for(demand)?;
            if self.running {
                self.control.set_active(false);
                self.status_session.clear();
                self.stop_worker();
            }
            self.demand = demand;
            self.refresh_platform_status()?;
            return Ok(());
        }
        let request =
            production_stream_request(&self.config, demand, self.control.host_capabilities())?;
        #[cfg(feature = "macos-capture-fixtures")]
        let prepared = demand
            .requested_extent()
            .map(|extent| self.prepare_worker(extent))
            .transpose()?
            .map(|prepared| self.stage_worker(prepared))
            .transpose()?;
        #[cfg(not(feature = "macos-capture-fixtures"))]
        let prepared = demand
            .requested_extent()
            .map(|extent| self.prepare_worker(extent))
            .map(|prepared| self.stage_worker(prepared))
            .transpose()?;
        if !self.running {
            self.control.begin_stream_request(request)?.wait()?;
            self.refresh_policy_for(demand)?;
            self.demand = demand;
            return Ok(());
        }
        let request = self.control.begin_stream_request(request)?;
        if let Some(prepared) = prepared {
            let session = if was_active {
                None
            } else {
                self.refresh_policy_for(demand)?;
                self.status.begin_session()?
            };
            if let Err(error) = request.wait() {
                if !was_active {
                    self.refresh_policy_for(self.demand)?;
                }
                return Err(error);
            }
            self.install_worker(prepared);
            if let Some(session) = session {
                self.status_session.store(session);
            }
            self.control.set_active(true);
        } else {
            request.wait()?;
        }
        self.demand = demand;
        self.refresh_platform_status()?;
        Ok(())
    }

    fn set_screen_renderer_execution_state(&mut self, state: ScreenRendererExecutionState) {
        self.telemetry.set_renderer_execution_state(state);
        let _ = self.refresh_platform_status();
    }

    fn set_screen_publication_hub(&mut self, hub: Arc<ScreenPublicationHub>) {
        self.exact.install_hub(hub);
    }

    fn screen_publication_resolution_revision(&self) -> u64 {
        self.exact.resolution_revision()
    }

    fn resolve_screen_publication_branch(
        &self,
        demand: &RegisteredScreenBranchDemand,
    ) -> anyhow::Result<Option<ResolvedScreenBranchDemand>> {
        let Some(source) = self.exact.source() else {
            tracing::debug!(
                shared = ?std::ptr::from_ref(self.exact.as_ref()),
                "exact branch unresolvable: no publication source installed"
            );
            return Ok(None);
        };
        let calibration = LedToneMapCalibration::try_new(
            self.config.target_led_white_x,
            self.config.target_led_white_y,
            self.config.target_led_reference_white_nits,
            self.config.target_led_peak_nits,
            self.config.exposure_ev,
        )?;
        let request = demand.request();
        let processing_profile = request
            .processing_profile()
            .as_ref()
            .clone()
            .with_led_tone_map(calibration);
        let calibrated = RegisteredScreenBranchDemand::new(
            ScreenPublicationRequest::new(
                request.selector().clone(),
                request.kind(),
                request.executor().clone(),
                request.extent(),
                request.aspect(),
                Arc::new(processing_profile),
            ),
            demand.requested_hz(),
        );
        resolve_macos_publication_branch_with_telemetry(&source, &calibrated, &self.telemetry)
    }

    fn owns_screen_publication_source(&self, source_id: &CaptureSourceId) -> bool {
        self.exact.owns_source(source_id)
    }

    fn begin_screen_publication_preparation(
        &mut self,
        ticket: ScreenWorkerPreparationTicket,
    ) -> anyhow::Result<ScreenWorkerPreparation> {
        let worker = self.worker.as_ref().ok_or_else(|| {
            anyhow!("macOS capture worker is unavailable for exact publication preparation")
        })?;
        let cancelled = Arc::new(AtomicBool::new(false));
        let (completion_tx, completion_rx) = oneshot::channel();
        worker
            .command_tx
            .send(WorkerCommand::PrepareExact {
                ticket,
                cancelled: Arc::clone(&cancelled),
                completion: completion_tx,
            })
            .map_err(|_| anyhow!("macOS capture worker rejected exact publication preparation"))?;
        worker.mailbox.wake();
        let abort_tx = worker.command_tx.clone();
        let abort_mailbox = worker.mailbox.clone();
        Ok(ScreenWorkerPreparation::with_abort(
            async move {
                completion_rx.await.map_err(|_| {
                    anyhow!("macOS capture worker exited during exact publication preparation")
                })?
            },
            move || {
                cancelled.store(true, Ordering::Release);
                let _ = abort_tx.send(WorkerCommand::ReapExact { completion: None });
                abort_mailbox.wake();
            },
        ))
    }

    fn begin_screen_publication_retirement(&mut self) -> Option<ScreenWorkerRetirement> {
        let worker = self.worker.as_ref()?;
        let (completion_tx, completion_rx) = oneshot::channel();
        if worker
            .command_tx
            .send(WorkerCommand::ReapExact {
                completion: Some(completion_tx),
            })
            .is_err()
        {
            return Some(ScreenWorkerRetirement::new(async {
                Err(anyhow!(
                    "macOS capture worker rejected exact publication retirement"
                ))
            }));
        }
        worker.mailbox.wake();
        Some(ScreenWorkerRetirement::new(async move {
            completion_rx.await.map_err(|_| {
                anyhow!("macOS capture worker exited during exact publication retirement")
            })?
        }))
    }

    fn reconfigure_screen_capture(&mut self, config: &CaptureConfig) -> anyhow::Result<()> {
        if !self.demand.is_active() {
            self.config.clone_from(config);
            return Ok(());
        }
        let request =
            production_stream_request(config, self.demand, self.control.host_capabilities())?;
        let prepared = self
            .demand
            .requested_extent()
            .map(|extent| {
                #[cfg(not(feature = "macos-capture-fixtures"))]
                let _ = extent;
                #[cfg(feature = "macos-capture-fixtures")]
                let mut analyzer = match self.compute_capacity_policy.analysis() {
                    Some(capacity) => {
                        ScreenCaptureInput::with_requested_extent_admission_and_compute_capacity(
                            config.clone(),
                            extent,
                            self.admission.clone(),
                            capacity,
                        )?
                    }
                    None => ScreenCaptureInput::with_requested_extent_and_admission(
                        config.clone(),
                        extent,
                        self.admission.clone(),
                    )?,
                };
                #[cfg(feature = "macos-capture-fixtures")]
                analyzer.start()?;
                Ok::<_, anyhow::Error>(PreparedWorker {
                    #[cfg(feature = "macos-capture-fixtures")]
                    analyzer,
                    #[cfg(feature = "macos-capture-fixtures")]
                    plane_pool: CapturePlanePool::with_admission_coordinator(
                        self.admission.clone(),
                    ),
                    target_fps: config.target_fps,
                })
            })
            .transpose()?
            .map(|prepared| self.stage_worker(prepared))
            .transpose()?;
        let request = self.control.begin_stream_request(request)?;
        request.wait()?;
        if self.running
            && let Some(prepared) = prepared
        {
            self.install_worker(prepared);
        }
        self.config.clone_from(config);
        Ok(())
    }

    fn reconfigure_screen_processing(&mut self, config: &CaptureConfig) -> anyhow::Result<()> {
        let next = LedToneMapCalibration::try_new(
            config.target_led_white_x,
            config.target_led_white_y,
            config.target_led_reference_white_nits,
            config.target_led_peak_nits,
            config.exposure_ev,
        )?;
        let current = LedToneMapCalibration::try_new(
            self.config.target_led_white_x,
            self.config.target_led_white_y,
            self.config.target_led_reference_white_nits,
            self.config.target_led_peak_nits,
            self.config.exposure_ev,
        )?;
        if current == next {
            return Ok(());
        }
        #[cfg(feature = "macos-capture-fixtures")]
        if let Some(worker) = self.worker.as_ref() {
            let (completion_tx, completion_rx) = mpsc::sync_channel(1);
            worker
                .command_tx
                .send(WorkerCommand::ReconfigureProcessing {
                    calibration: next,
                    completion: completion_tx,
                })
                .map_err(|_| anyhow!("macOS capture worker rejected processing reconfiguration"))?;
            worker.mailbox.wake();
            completion_rx.recv().map_err(|_| {
                anyhow!("macOS capture worker exited during processing reconfiguration")
            })??;
        }
        self.config.target_led_white_x = next.target_white_x();
        self.config.target_led_white_y = next.target_white_y();
        self.config.target_led_reference_white_nits = next.target_reference_white_nits();
        self.config.target_led_peak_nits = next.target_peak_nits();
        self.config.exposure_ev = next.exposure_ev();
        self.exact.advance_resolution_revision();
        Ok(())
    }

    fn reselect_screen_source(&mut self) -> anyhow::Result<()> {
        self.present_picker()
    }

    fn screen_authorization_action(&self) -> Option<ProtectedSourceAuthorizationAction> {
        let control = Arc::clone(&self.control);
        Some(ProtectedSourceAuthorizationAction::new(
            Arc::new(move || {
                control.request_authorization();
                Ok(control.authorization() == MacosScreenAuthorizationState::Authorized)
            }),
            protected_action_identity(self.owner, false),
        ))
    }

    fn screen_source_picker_action(&self) -> Option<ScreenSourcePickerAction> {
        let control = Arc::clone(&self.control);
        Some(ScreenSourcePickerAction::new(
            Arc::new(move || control.present_picker()),
            protected_action_identity(self.owner, true),
        ))
    }

    #[cfg(target_os = "macos")]
    fn diagnostic_artifact_action(&self) -> Option<SourceDiagnosticArtifactAction> {
        let control = Arc::clone(&self.control);
        Some(Arc::new(move || {
            control
                .capture_screenshot_reference()
                .map(|receiver| Box::new(receiver) as crate::input::SourceDiagnosticArtifact)
        }))
    }
}
