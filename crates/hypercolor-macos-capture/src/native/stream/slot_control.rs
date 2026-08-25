use super::{
    Arc, CandidatePreparationFailure, CandidateReservation, CandidateStageIdentity,
    CaptureActivation, Instant, InterruptedRestagePlan, MACOS_NATIVE_STOP_TIMEOUT,
    MACOS_NATIVE_STREAM_TRANSACTION_TIMEOUT, MacosCaptureError, MacosProtectedSourceState,
    MacosScreenshotReferenceCapability, MacosStreamRequest, MacosStreamRequestTransaction,
    NativeSelectionFilter, NativeStream, PendingStreamRequest, PoolReservationFactory, Retained,
    SCStream, ScreenshotFilterHandle, ScreenshotTransactionSnapshot, StreamSlot, lock,
    stream_request_transaction,
};
#[cfg(test)]
use super::{MacosCaptureSelection, MacosNativeTransactionPhase};

impl StreamSlot {
    pub(super) fn active_identity(&self) -> Option<(Arc<str>, u64)> {
        lock(&self.state)
            .current
            .as_ref()
            .map(|current| (Arc::clone(&current.source_id), current.epoch()))
    }

    pub(super) fn has_selection(&self) -> bool {
        let state = lock(&self.state);
        state.pending_selection.is_some() || state.selected_filter.is_some()
    }

    #[cfg(test)]
    pub(super) fn clear_selection(&self) -> Result<(), MacosCaptureError> {
        let _lifecycle = lock(&self.lifecycle_start);
        let mut state = lock(&self.state);
        state.selection_revision = state
            .selection_revision
            .checked_add(1)
            .ok_or(MacosCaptureError::SequenceExhausted)?;
        state.lifecycle_revision = state
            .lifecycle_revision
            .checked_add(1)
            .ok_or(MacosCaptureError::SequenceExhausted)?;
        state.selected_filter = None;
        state.pending_selection = None;
        state.pending_interruption = None;
        let candidate_settlement = Self::cancel_candidate_completion(&mut state);
        state.pending_request = None;
        state.staging_epoch = None;
        state.candidate_epoch = None;
        state.inactive_epochs.clear();
        state.terminal_epochs.clear();
        drop(state);
        Self::finish_replaced_candidate(candidate_settlement);
        self.shared
            .set_unconfirmed_selection(MacosCaptureSelection::None);
        Ok(())
    }

    pub(super) fn screenshot_capability(
        &self,
    ) -> Result<MacosScreenshotReferenceCapability, MacosCaptureError> {
        let state = lock(&self.state);
        let Some(current) = state.current.as_ref() else {
            return Ok(MacosScreenshotReferenceCapability::PendingFirstFrame);
        };
        self.capability_for_current(current)
    }

    pub(super) fn screenshot_snapshot(
        &self,
    ) -> Result<ScreenshotTransactionSnapshot, MacosCaptureError> {
        let state = lock(&self.state);
        let current = state
            .current
            .as_ref()
            .ok_or(MacosCaptureError::ScreenshotCapabilityPending)?;
        let capability = self.capability_for_current(current)?;
        Ok(ScreenshotTransactionSnapshot {
            filter: ScreenshotFilterHandle::Native(current.filter.clone()),
            source_id: Arc::clone(&current.source_id),
            generation: current.epoch(),
            selection_revision: state.selection_revision,
            capability,
        })
    }

    pub(super) fn capability_for_current(
        &self,
        current: &NativeStream,
    ) -> Result<MacosScreenshotReferenceCapability, MacosCaptureError> {
        if !self.shared.tahoe.screenshot_api.is_present() {
            return Err(MacosCaptureError::TahoePlatformDefect(
                "Tahoe screenshot API",
            ));
        }
        if !self.shared.tahoe.content_tone_mapping_info.is_present() {
            return Err(MacosCaptureError::TahoePlatformDefect(
                "Core Graphics Tahoe tone mapping",
            ));
        }
        crate::screenshot::require_tahoe_reference_output_symbols()?;
        let capability = self
            .shared
            .tahoe_selection_for(&current.source_id, current.epoch())
            .ok_or(MacosCaptureError::ScreenshotCapabilityPending)?;
        if capability.hdr_capture {
            if !capability.dual_range_screenshots {
                return Err(MacosCaptureError::TahoePlatformDefect(
                    "paired SDR and HDR screenshots",
                ));
            }
            Ok(MacosScreenshotReferenceCapability::PairedSdrHdr {
                source_id: capability.source_id,
                generation: capability.capture_session_generation,
            })
        } else {
            Ok(MacosScreenshotReferenceCapability::SdrOnly {
                source_id: capability.source_id,
                generation: capability.capture_session_generation,
            })
        }
    }

    pub(super) fn request(&self) -> MacosStreamRequest {
        let state = lock(&self.state);
        state
            .pending_request
            .as_ref()
            .map_or(state.request, |pending| pending.request)
    }

    pub(super) fn committed_request(&self) -> MacosStreamRequest {
        lock(&self.state).request
    }

    pub(super) fn set_request(
        self: &Arc<Self>,
        request: MacosStreamRequest,
        reserve_pool: &PoolReservationFactory,
    ) -> Result<MacosStreamRequestTransaction, MacosCaptureError> {
        let (transaction, reservation) = self.begin_request_candidate(request)?;
        if let Some(reservation) = reservation
            && let Err(failure) =
                self.prepare_and_start_candidate(reservation, request, reserve_pool)
        {
            let error = failure.error.clone();
            self.finalize_candidate_preparation_failure(failure, None);
            return Err(error);
        }
        Ok(transaction)
    }

    pub(super) fn begin_request_candidate(
        self: &Arc<Self>,
        request: MacosStreamRequest,
    ) -> Result<(MacosStreamRequestTransaction, Option<CandidateReservation>), MacosCaptureError>
    {
        {
            let _lifecycle = lock(&self.lifecycle_start);
            let state = lock(&self.state);
            if state.pending_request.is_some() {
                return Err(MacosCaptureError::CaptureWorkerStartFailed(
                    "another stream request transaction is still pending".to_owned(),
                ));
            }
            if state.request == request {
                let generation = self.shared.current_epoch();
                let (transaction, completion) = stream_request_transaction(
                    generation,
                    Instant::now() + MACOS_NATIVE_STREAM_TRANSACTION_TIMEOUT,
                );
                if let Some(settlement) = completion.claim(Ok(())) {
                    settlement.publish();
                }
                return Ok((transaction, None));
            }
        }
        let epoch = self.allocate_epoch()?;
        let (transaction, completion) = stream_request_transaction(
            epoch,
            Instant::now() + MACOS_NATIVE_STREAM_TRANSACTION_TIMEOUT,
        );
        let reservation = self.reserve_candidate_stage(
            epoch,
            request,
            None,
            None,
            Some(PendingStreamRequest {
                epoch,
                request,
                completion: completion.clone(),
            }),
        )?;
        let cancel_streams = Arc::downgrade(self);
        completion.set_cancel(move |generation| {
            if let Some(streams) = cancel_streams.upgrade() {
                streams.cancel_candidate_transaction(generation);
            }
        });
        Ok((transaction, reservation))
    }

    #[cfg(test)]
    pub(super) fn begin_request_candidate_fixture(
        self: &Arc<Self>,
        request: MacosStreamRequest,
    ) -> Result<(MacosStreamRequestTransaction, Option<NativeStream>), MacosCaptureError> {
        let (transaction, reservation) = self.begin_request_candidate(request)?;
        let Some(reservation) = reservation else {
            return Ok((transaction, None));
        };
        let CandidateReservation {
            stage,
            replaced,
            replaced_settlement,
            ..
        } = reservation;
        Self::finish_replaced_candidate(replaced_settlement);
        if !self.arm_candidate_deadline(
            stage.epoch,
            MacosNativeTransactionPhase::StreamStart,
            MACOS_NATIVE_STREAM_TRANSACTION_TIMEOUT,
        )? {
            return Err(MacosCaptureError::CaptureWorkerStartFailed(
                "fixture request deadline was superseded before start".to_owned(),
            ));
        }
        if !self.start_candidate_fixture(stage) {
            return Err(MacosCaptureError::CaptureWorkerStartFailed(
                "fixture request candidate was superseded before start".to_owned(),
            ));
        }
        Ok((transaction, replaced))
    }

    pub(super) fn current_stream(&self) -> Option<Retained<SCStream>> {
        lock(&self.state)
            .current
            .as_ref()
            .map(|current| current.stream.clone())
    }

    pub(super) fn stage_interrupted_recovery(
        self: &Arc<Self>,
        plan: InterruptedRestagePlan,
    ) -> Result<bool, CandidatePreparationFailure> {
        let lifecycle_revision = lock(&self.state).lifecycle_revision;
        let epoch = self
            .allocate_epoch()
            .map_err(|error| CandidatePreparationFailure {
                stage: CandidateStageIdentity {
                    epoch: plan.recovery.interrupted_epoch,
                    selection_revision: plan.recovery.selection_revision,
                    lifecycle_revision,
                    predecessor_epoch: None,
                },
                error,
                settlement: None,
            })?;
        self.stage_candidate_with_selection(
            Some(plan.selection_filter),
            plan.request,
            &plan.reserve_pool,
            epoch,
            Some(plan.recovery),
            None,
        )
    }

    pub(super) fn begin_capture_activation(&self) -> Result<CaptureActivation, MacosCaptureError> {
        let _lifecycle = lock(&self.lifecycle_start);
        let mut state = lock(&self.state);
        if self.shared.capture_active() {
            return Ok(CaptureActivation::Unchanged);
        }
        let Some(selection_filter) = state.selected_filter.clone() else {
            self.shared.set_capture_active(true);
            return Ok(CaptureActivation::NeedsSelection);
        };
        let epoch = self.allocate_epoch()?;
        let request = state
            .pending_request
            .as_ref()
            .map_or(state.request, |pending| pending.request);
        let reservation =
            self.reserve_selection_candidate_locked(&mut state, epoch, request, selection_filter)?;
        self.shared.set_capture_active(true);
        Ok(CaptureActivation::Candidate {
            reservation: Box::new(reservation),
            request,
        })
    }

    pub(super) fn set_capture_active(&self, active: bool) -> bool {
        let _lifecycle = lock(&self.lifecycle_start);
        if self.shared.set_capture_active(active) == active {
            return false;
        }
        if active {
            return true;
        }
        self.shared.disable_picker_callbacks();
        let source_settlement = self.cancel_source_transaction_locked();
        let (current, candidate, selection, candidate_settlement, diagnostic_settlement) = {
            let mut state = lock(&self.state);
            let diagnostic_settlement = self
                .shared
                .claim_restart_diagnostic_completion(MacosProtectedSourceState::Failed);
            state.selection_revision = state.selection_revision.saturating_add(1);
            state.lifecycle_revision = state.lifecycle_revision.saturating_add(1);
            state.pending_interruption = None;
            let candidate_settlement = Self::cancel_candidate_completion(&mut state);
            state.staging_epoch = None;
            state.pending_request = None;
            state.candidate_epoch = None;
            state.inactive_epochs.clear();
            state.terminal_epochs.clear();
            #[cfg(test)]
            {
                state.fixture_candidate_epoch = None;
                state.fixture_current_epoch = None;
            }
            let mut selection = state.pending_selection.take().map(|pending| {
                let selection = pending.selection_filter.selection.clone();
                state.selected_filter = Some(pending.selection_filter);
                selection
            });
            if state.current.is_none()
                && state.selected_filter.is_none()
                && let Some(candidate) = state.candidate.as_ref()
            {
                let selection_filter = NativeSelectionFilter {
                    filter: candidate.filter.clone(),
                    selection: candidate.selection.clone(),
                    source_id: Arc::clone(&candidate.source_id),
                };
                selection = Some(selection_filter.selection.clone());
                state.selected_filter = Some(selection_filter);
            }
            (
                state.current.take(),
                state.candidate.take(),
                selection,
                candidate_settlement,
                diagnostic_settlement,
            )
        };
        self.shared.activate_epoch(0);
        self.shared.clear_tahoe_selection();
        if let Some(selection) = selection {
            self.shared.set_unconfirmed_selection(selection);
        }
        drop(_lifecycle);
        if let Some(candidate) = candidate {
            self.stop_stream(candidate);
        }
        if let Some(current) = current {
            self.stop_stream(current);
        }
        Self::finish_replaced_candidate(candidate_settlement);
        if let Some(settlement) = source_settlement {
            settlement.publish();
        }
        if let Some(settlement) = diagnostic_settlement {
            settlement.publish();
        }
        true
    }

    pub(super) fn stop_stream(&self, stream: NativeStream) {
        let start_completion = stream.start_completion.clone();
        let shared = Arc::clone(&self.shared);
        let stop_shared = Arc::clone(&shared);
        let timeout_shared = Arc::clone(&shared);
        self.native_lifecycle.retire(
            stream,
            start_completion,
            Instant::now() + MACOS_NATIVE_STOP_TIMEOUT,
            move |stream, stop_completion| {
                stream.worker.close();
                stream.control.enqueue_stop(stop_shared, stop_completion);
                if let Err(error) = stream.finish_worker_retirement() {
                    shared.record_retirement_error(&error);
                }
            },
            move || {
                timeout_shared
                    .record_retirement_error(&MacosCaptureError::StreamStopCompletionLost);
            },
        );
    }

    pub(super) fn retire_stream_after_native_error(&self, stream: NativeStream) {
        let start_completion = stream.start_completion.clone();
        let shared = Arc::clone(&self.shared);
        self.native_lifecycle
            .retire_without_native_stop(stream, start_completion, move |stream| {
                if let Err(error) = stream.finish_worker_retirement() {
                    shared.counters.record_drop(&error);
                }
            });
    }

    pub(super) fn retire_unstarted_stream(&self, stream: NativeStream) {
        let start_completion = stream.start_completion.clone();
        drop(start_completion.witness());
        let shared = Arc::clone(&self.shared);
        self.native_lifecycle
            .retire_without_native_stop(stream, start_completion, move |stream| {
                if let Err(error) = stream.finish_worker_retirement() {
                    shared.counters.record_drop(&error);
                }
            });
    }
}
