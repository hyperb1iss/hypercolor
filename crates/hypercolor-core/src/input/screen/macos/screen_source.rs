use std::sync::Arc;
#[cfg(feature = "macos-capture-fixtures")]
use std::sync::mpsc;

use super::SourceDiagnosticArtifactAction;
#[cfg(feature = "macos-capture-fixtures")]
use super::{
    BackendWorkerCommand, CapturePlanePool, InputSource, ScreenCaptureInput, WorkerCommand,
};
use super::{
    CaptureConfig, CaptureSourceId, LedToneMapCalibration, MacosScreenCaptureInput, PreparedWorker,
    ProtectedSourceAuthorizationAction, RegisteredScreenBranchDemand, ResolvedScreenBranchDemand,
    ScreenCaptureDemand, ScreenPublicationHub, ScreenRendererExecutionState, ScreenSource,
    ScreenSourcePickerAction, ScreenWorkerPreparation, ScreenWorkerPreparationTicket,
    ScreenWorkerRetirement, SourceCapabilityContext, production_stream_request,
    protected_action_identity,
};
use crate::input::screen::planner::ScreenNativeExecutionPolicy;
#[cfg(feature = "macos-capture-fixtures")]
use anyhow::anyhow;
use hypercolor_macos_capture::MacosScreenAuthorizationState;
use hypercolor_macos_input::{MacosCapabilityOwner, MacosDaemonOwnerConflict};

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
        self.shell.adapter.settings().demand()
    }

    /// ScreenCaptureKit frames are IOSurfaces that stay on the GPU; the
    /// renderer either consumes them natively or waits for a target.
    fn native_execution_policy(&self) -> ScreenNativeExecutionPolicy {
        ScreenNativeExecutionPolicy::Required
    }

    fn set_screen_capture_demand(&mut self, demand: ScreenCaptureDemand) -> anyhow::Result<()> {
        let snapshot = self.shell.adapter.settings().snapshot();
        let was_active = snapshot.demand.is_active();
        if !demand.is_active() {
            self.refresh_policy_for(demand)?;
            if self.shell.running {
                self.control.set_active(false);
                self.shell.status_session.clear();
                self.stop_worker();
            }
            let mut settings = self.shell.adapter.settings().lock();
            *settings.demand_mut() = demand;
            settings.commit();
            self.refresh_platform_status()?;
            return Ok(());
        }
        let request =
            production_stream_request(&snapshot.config, demand, self.control.host_capabilities())?;
        #[cfg(feature = "macos-capture-fixtures")]
        let prepared = Some(self.stage_worker(self.prepare_worker()?)?);
        #[cfg(not(feature = "macos-capture-fixtures"))]
        let prepared = Some(self.stage_worker(self.prepare_worker())?);
        if !self.shell.running {
            self.control.begin_stream_request(request)?.wait()?;
            self.refresh_policy_for(demand)?;
            let mut settings = self.shell.adapter.settings().lock();
            *settings.demand_mut() = demand;
            settings.commit();
            return Ok(());
        }
        let request = self.control.begin_stream_request(request)?;
        if let Some(prepared) = prepared {
            let session = if was_active {
                None
            } else {
                self.refresh_policy_for(demand)?;
                self.shell.status.begin_session()?
            };
            if let Err(error) = request.wait() {
                if !was_active {
                    self.refresh_policy_for(snapshot.demand)?;
                }
                return Err(error);
            }
            self.install_worker(prepared);
            if let Some(session) = session {
                self.shell.status_session.store(session);
            }
            self.control.set_active(true);
        } else {
            request.wait()?;
        }
        let mut settings = self.shell.adapter.settings().lock();
        *settings.demand_mut() = demand;
        settings.commit();
        self.refresh_platform_status()?;
        Ok(())
    }

    fn set_screen_renderer_execution_state(&mut self, state: ScreenRendererExecutionState) {
        self.telemetry.set_renderer_execution_state(state);
        let _ = self.refresh_platform_status();
    }

    fn set_screen_publication_hub(&mut self, hub: Arc<ScreenPublicationHub>) {
        self.shell.adapter.install_publication_hub(hub);
    }

    fn screen_publication_resolution_revision(&self) -> u64 {
        self.shell.adapter.exact_resolution_revision()
    }

    fn resolve_screen_publication_branch(
        &self,
        demand: &RegisteredScreenBranchDemand,
    ) -> anyhow::Result<Option<ResolvedScreenBranchDemand>> {
        self.shell.adapter.resolve_exact_publication_branch(demand)
    }

    fn owns_screen_publication_source(&self, source_id: &CaptureSourceId) -> bool {
        self.shell.adapter.owns_exact_source(source_id)
    }

    fn begin_screen_publication_preparation(
        &mut self,
        ticket: ScreenWorkerPreparationTicket,
    ) -> anyhow::Result<ScreenWorkerPreparation> {
        self.shell.adapter.begin_exact_preparation(ticket)
    }

    fn begin_screen_publication_retirement(&mut self) -> Option<ScreenWorkerRetirement> {
        self.shell.adapter.begin_exact_retirement()
    }

    fn reconfigure_screen_capture(&mut self, config: &CaptureConfig) -> anyhow::Result<()> {
        let snapshot = self.shell.adapter.settings().snapshot();
        if !snapshot.demand.is_active() {
            let mut settings = self.shell.adapter.settings().lock();
            settings.config_mut().clone_from(config);
            settings.commit();
            return Ok(());
        }
        let request =
            production_stream_request(config, snapshot.demand, self.control.host_capabilities())?;
        #[cfg(feature = "macos-capture-fixtures")]
        let prepared = {
            let mut analyzer = match self.compute_capacity_policy.analysis() {
                Some(capacity) => {
                    ScreenCaptureInput::with_requested_extent_admission_and_compute_capacity(
                        config.clone(),
                        super::fixture_analysis_extent(),
                        self.admission.clone(),
                        capacity,
                    )?
                }
                None => ScreenCaptureInput::with_requested_extent_and_admission(
                    config.clone(),
                    super::fixture_analysis_extent(),
                    self.admission.clone(),
                )?,
            };
            analyzer.start()?;
            Some(self.stage_worker(PreparedWorker {
                analyzer,
                plane_pool: CapturePlanePool::with_admission_coordinator(self.admission.clone()),
                target_fps: config.target_fps,
            })?)
        };
        #[cfg(not(feature = "macos-capture-fixtures"))]
        let prepared = Some(self.stage_worker(PreparedWorker {
            target_fps: config.target_fps,
        })?);
        let request = self.control.begin_stream_request(request)?;
        request.wait()?;
        if self.shell.running
            && let Some(prepared) = prepared
        {
            self.install_worker(prepared);
        }
        let mut settings = self.shell.adapter.settings().lock();
        settings.config_mut().clone_from(config);
        settings.commit();
        Ok(())
    }

    fn reconfigure_screen_processing(&mut self, config: &CaptureConfig) -> anyhow::Result<()> {
        let snapshot = self.shell.adapter.settings().snapshot();
        config.exact_processing_profile(&super::super::ScreenProcessingProfile::default())?;
        if !snapshot.config.processing_controls_differ(config) {
            return Ok(());
        }
        let next = LedToneMapCalibration::try_new(
            config.target_led_white_x,
            config.target_led_white_y,
            config.target_led_reference_white_nits,
            config.target_led_peak_nits,
            config.exposure_ev,
        )?;
        let current = LedToneMapCalibration::try_new(
            snapshot.config.target_led_white_x,
            snapshot.config.target_led_white_y,
            snapshot.config.target_led_reference_white_nits,
            snapshot.config.target_led_peak_nits,
            snapshot.config.exposure_ev,
        )?;
        let calibration_changed = current != next;
        if calibration_changed {
            #[cfg(feature = "macos-capture-fixtures")]
            if let Some(worker) = self.shell.adapter.active_worker() {
                let (completion_tx, completion_rx) = mpsc::sync_channel(1);
                worker
                    .command_tx
                    .send(WorkerCommand::Backend(
                        BackendWorkerCommand::ReconfigureProcessing {
                            calibration: next,
                            completion: completion_tx,
                        },
                    ))
                    .map_err(|_| {
                        anyhow!("macOS capture worker rejected processing reconfiguration")
                    })?;
                worker.mailbox.wake();
                completion_rx.recv().map_err(|_| {
                    anyhow!("macOS capture worker exited during processing reconfiguration")
                })??;
            }
        }
        let mut settings = self.shell.adapter.settings().lock();
        settings.config_mut().copy_processing_controls_from(config);
        settings.commit();
        self.shell.adapter.advance_exact_resolution_revision();
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

    fn diagnostic_artifact_action(&self) -> Option<SourceDiagnosticArtifactAction> {
        let control = Arc::clone(&self.control);
        Some(Arc::new(move || {
            control
                .capture_screenshot_reference()
                .map(|receiver| Box::new(receiver) as crate::input::SourceDiagnosticArtifact)
        }))
    }
}
