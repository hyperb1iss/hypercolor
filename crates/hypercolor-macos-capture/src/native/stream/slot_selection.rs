use super::{
    Arc, CandidatePreparationFailure, CandidateReservation, CandidateStage, CandidateStageIdentity,
    FilterAcceptance, InterruptedRestage, MacosCaptureError, MacosNativeTransactionError,
    MacosProtectedSourceState, MacosStreamRequest, NativeSelectionFilter, NativeStream,
    PendingSelectionFilter, PendingStreamRequest, PoolReservationFactory, SourceResolution,
    StreamSlot, StreamState, TransactionSettlement, lock,
};

impl StreamSlot {
    pub(super) fn reserve_selection_candidate_locked(
        &self,
        state: &mut StreamState,
        epoch: u64,
        candidate_request: MacosStreamRequest,
        selection_filter: NativeSelectionFilter,
    ) -> Result<CandidateReservation, MacosCaptureError> {
        let authoritative_request = state
            .pending_request
            .as_ref()
            .map_or(state.request, |pending| pending.request);
        if candidate_request != authoritative_request {
            return Err(MacosCaptureError::CaptureWorkerStartFailed(
                "candidate request snapshot does not match the authoritative stream request"
                    .to_owned(),
            ));
        }
        let selection_revision = state
            .selection_revision
            .checked_add(1)
            .ok_or(MacosCaptureError::SequenceExhausted)?;
        let lifecycle_revision = state
            .lifecycle_revision
            .checked_add(1)
            .ok_or(MacosCaptureError::SequenceExhausted)?;
        state.selection_revision = selection_revision;
        state.lifecycle_revision = lifecycle_revision;
        state.pending_interruption = None;
        let request = state.pending_request.take().map(|mut pending| {
            pending.epoch = epoch;
            pending
        });
        let request_identity = request.as_ref().map(PendingStreamRequest::identity);
        let replaced_settlement =
            Self::install_candidate_completion(state, epoch, request.as_ref());
        state.pending_request = request;
        state.pending_selection = Some(PendingSelectionFilter {
            epoch,
            selection_revision: state.selection_revision,
            selection_filter: selection_filter.clone(),
        });
        let stage = CandidateStage {
            epoch,
            selection_revision: state.selection_revision,
            lifecycle_revision,
            predecessor_epoch: Self::current_epoch(state),
            recovery_current_epoch: None,
            recovery: None,
            request: request_identity,
        };
        state.staging_epoch = Some(epoch);
        if let Some(replaced_epoch) = state.candidate_epoch {
            Self::forget_epoch_activity(state, replaced_epoch);
        }
        state.candidate_epoch = None;
        Ok(CandidateReservation {
            stage,
            selection_filter,
            replaced: state.candidate.take(),
            replaced_settlement,
        })
    }

    pub(super) fn accept_selection_filter_with(
        &self,
        selection_filter: NativeSelectionFilter,
        candidate_request: MacosStreamRequest,
        epoch: u64,
        resolution: SourceResolution,
        picker: bool,
        accepted: impl FnOnce(),
    ) -> Result<FilterAcceptance, MacosCaptureError> {
        self.accept_selection_filter_with_hooks(
            selection_filter,
            candidate_request,
            epoch,
            resolution,
            picker,
            (|| {}, accepted),
        )
    }

    pub(super) fn accept_selection_filter_with_hooks(
        &self,
        selection_filter: NativeSelectionFilter,
        candidate_request: MacosStreamRequest,
        epoch: u64,
        resolution: SourceResolution,
        picker: bool,
        hooks: (impl FnOnce(), impl FnOnce()),
    ) -> Result<FilterAcceptance, MacosCaptureError> {
        let (before_transition, accepted) = hooks;
        before_transition();
        let _lifecycle = lock(&self.lifecycle_start);
        let mut state = lock(&self.state);
        if !self.resolution_is_current(&state, &resolution) {
            return Ok(FilterAcceptance::Stale);
        }
        if picker && !self.shared.consume_picker_resolution(&resolution) {
            return Ok(FilterAcceptance::Stale);
        }
        let (acceptance, stored_selection) = if self.shared.capture_active() {
            let request = state
                .pending_request
                .as_ref()
                .map_or(state.request, |pending| pending.request);
            let reservation = self.reserve_selection_candidate_locked(
                &mut state,
                epoch,
                candidate_request,
                selection_filter,
            )?;
            (
                FilterAcceptance::Candidate {
                    reservation: Box::new(reservation),
                    request,
                },
                None,
            )
        } else {
            let selection_revision = state
                .selection_revision
                .checked_add(1)
                .ok_or(MacosCaptureError::SequenceExhausted)?;
            let lifecycle_revision = state
                .lifecycle_revision
                .checked_add(1)
                .ok_or(MacosCaptureError::SequenceExhausted)?;
            state.selection_revision = selection_revision;
            state.lifecycle_revision = lifecycle_revision;
            state.pending_interruption = None;
            state.staging_epoch = None;
            state.pending_selection = None;
            state.inactive_epochs.clear();
            state.terminal_epochs.clear();
            state.candidate_epoch = None;
            let selection = selection_filter.selection.clone();
            state.selected_filter = Some(selection_filter);
            let replaced = state.candidate.take();
            (FilterAcceptance::Stored(replaced), Some(selection))
        };
        if let SourceResolution::Diagnostic(diagnostic) = &resolution {
            self.shared.record_filter_enumerated(diagnostic, epoch);
        }
        drop(state);
        if let Some(selection) = stored_selection {
            self.shared.set_unconfirmed_selection(selection);
            self.shared.set_status(MacosProtectedSourceState::ReadyIdle);
        }
        accepted();
        Ok(acceptance)
    }

    pub(super) fn accept_selection_filter(
        &self,
        selection_filter: NativeSelectionFilter,
        candidate_request: MacosStreamRequest,
        epoch: u64,
        resolution: SourceResolution,
        picker: bool,
    ) -> Result<FilterAcceptance, MacosCaptureError> {
        self.accept_selection_filter_with(
            selection_filter,
            candidate_request,
            epoch,
            resolution,
            picker,
            || {},
        )
    }

    pub(super) fn resolution_is_current(
        &self,
        state: &StreamState,
        resolution: &SourceResolution,
    ) -> bool {
        self.shared.source_resolution_is_current(resolution)
            && match resolution {
                SourceResolution::General(_) => true,
                SourceResolution::Diagnostic(diagnostic) => {
                    state.selection_revision == diagnostic.attempt.selection_revision
                }
            }
    }

    pub(super) fn finalize_picker_cancel(&self, resolution: &SourceResolution) -> bool {
        let _lifecycle = lock(&self.lifecycle_start);
        let state = lock(&self.state);
        if !self.resolution_is_current(&state, resolution)
            || !self.shared.consume_picker_resolution(resolution)
        {
            return false;
        }
        let needs_selection = Self::current_epoch(&state).is_none()
            && state.pending_selection.is_none()
            && state.selected_filter.is_none();
        drop(state);
        if needs_selection {
            self.shared
                .set_status(MacosProtectedSourceState::NeedsSelection);
        }
        true
    }

    pub(super) fn finalize_session_scoped_resolution(&self, resolution: &SourceResolution) -> bool {
        let _lifecycle = lock(&self.lifecycle_start);
        let state = lock(&self.state);
        if !self.resolution_is_current(&state, resolution) {
            return false;
        }
        drop(state);
        self.shared
            .set_status(MacosProtectedSourceState::NeedsSelection);
        true
    }

    pub(super) fn finalize_resolution_error(
        &self,
        resolution: &SourceResolution,
        consume_picker: bool,
        error: MacosCaptureError,
    ) -> bool {
        let _lifecycle = lock(&self.lifecycle_start);
        let state = lock(&self.state);
        if !self.resolution_is_current(&state, resolution)
            || (consume_picker && !self.shared.consume_picker_resolution(resolution))
        {
            return false;
        }
        if let SourceResolution::Diagnostic(diagnostic) = resolution {
            self.shared.record_non_stream_diagnostic_failure(
                diagnostic,
                MacosProtectedSourceState::Failed,
            );
        }
        let preserve_current = Self::current_epoch(&state).is_some();
        let preserve_selection =
            state.pending_selection.is_some() || state.selected_filter.is_some();
        let preserve_status =
            preserve_current || state.candidate_epoch.is_some() || state.staging_epoch.is_some();
        let status = (!preserve_status).then_some({
            if preserve_selection {
                MacosProtectedSourceState::ReadyIdle
            } else if matches!(error, MacosCaptureError::DisplaySourceUnavailable(_)) {
                MacosProtectedSourceState::NeedsSelection
            } else {
                MacosProtectedSourceState::Failed
            }
        });
        drop(state);
        if let Some(status) = status {
            self.shared.set_status(status);
        }
        if preserve_current || preserve_selection {
            self.shared.publish_recoverable_error(error);
        } else {
            self.shared.publish_error(error);
        }
        true
    }

    pub(super) fn finalize_picker_failure(
        &self,
        resolution: &SourceResolution,
        error: MacosCaptureError,
    ) -> bool {
        self.finalize_resolution_error(resolution, true, error)
    }

    pub(super) fn finalize_candidate_preparation_failure(
        &self,
        failure: CandidatePreparationFailure,
        resolution: Option<&SourceResolution>,
    ) -> bool {
        self.finalize_candidate_preparation_failure_with(failure, resolution, || {})
    }

    pub(super) fn finalize_candidate_preparation_failure_with(
        &self,
        mut failure: CandidatePreparationFailure,
        resolution: Option<&SourceResolution>,
        before_finalization: impl FnOnce(),
    ) -> bool {
        before_finalization();
        let finalized = (|| {
            let _lifecycle = lock(&self.lifecycle_start);
            let mut state = lock(&self.state);
            if resolution.is_some_and(|resolution| !self.resolution_is_current(&state, resolution))
            {
                return false;
            }
            let failed_stage_cleared = state.staging_epoch != Some(failure.stage.epoch)
                && state.candidate_epoch != Some(failure.stage.epoch)
                && state
                    .pending_selection
                    .as_ref()
                    .is_none_or(|pending| pending.epoch != failure.stage.epoch);
            let lifecycle_matches = failed_stage_cleared
                && state.selection_revision == failure.stage.selection_revision
                && state.lifecycle_revision == failure.stage.lifecycle_revision
                && Self::current_epoch(&state) == failure.stage.predecessor_epoch
                && state.staging_epoch.is_none()
                && state.candidate_epoch.is_none();
            if !lifecycle_matches {
                return false;
            }
            let Some(lifecycle_revision) = state.lifecycle_revision.checked_add(1) else {
                return false;
            };
            if let Some(SourceResolution::Diagnostic(diagnostic)) = resolution {
                self.shared.record_non_stream_diagnostic_failure(
                    diagnostic,
                    MacosProtectedSourceState::Failed,
                );
            }
            let current_epoch = Self::current_epoch(&state);
            let current_inactive =
                current_epoch.is_some_and(|epoch| state.inactive_epochs.contains(&epoch));
            let preserve_current = current_epoch.is_some();
            let preserve_selection = state.selected_filter.is_some();
            let status = if preserve_current {
                if current_inactive {
                    MacosProtectedSourceState::NeedsSelection
                } else {
                    MacosProtectedSourceState::Live
                }
            } else if preserve_selection {
                MacosProtectedSourceState::ReadyIdle
            } else if matches!(
                &failure.error,
                MacosCaptureError::DisplaySourceUnavailable(_)
            ) {
                MacosProtectedSourceState::NeedsSelection
            } else {
                MacosProtectedSourceState::Failed
            };
            state.lifecycle_revision = lifecycle_revision;
            drop(state);
            self.shared.set_status(status);
            if preserve_current || preserve_selection {
                self.shared.publish_recoverable_error(failure.error.clone());
            } else {
                self.shared.publish_error(failure.error.clone());
            }
            true
        })();
        if let Some(settlement) = failure.settlement.take() {
            (*settlement).publish();
        }
        finalized
    }

    pub(super) fn stage_candidate_with_selection(
        self: &Arc<Self>,
        selection_filter: Option<NativeSelectionFilter>,
        request: MacosStreamRequest,
        reserve_pool: &PoolReservationFactory,
        epoch: u64,
        recovery: Option<InterruptedRestage>,
        request_transaction: Option<PendingStreamRequest>,
    ) -> Result<bool, CandidatePreparationFailure> {
        let failure_stage = {
            let state = lock(&self.state);
            CandidateStageIdentity {
                epoch,
                selection_revision: recovery.map_or(state.selection_revision, |recovery| {
                    recovery.selection_revision
                }),
                lifecycle_revision: state.lifecycle_revision,
                predecessor_epoch: recovery
                    .is_none()
                    .then(|| Self::current_epoch(&state))
                    .flatten(),
            }
        };
        let Some(reservation) = self
            .reserve_candidate_stage(
                epoch,
                request,
                selection_filter,
                recovery,
                request_transaction,
            )
            .map_err(|error| CandidatePreparationFailure {
                stage: failure_stage,
                error,
                settlement: None,
            })?
        else {
            return Ok(false);
        };
        self.prepare_and_start_candidate(reservation, request, reserve_pool)
    }

    pub(super) fn prepare_and_start_candidate(
        self: &Arc<Self>,
        reservation: CandidateReservation,
        request: MacosStreamRequest,
        reserve_pool: &PoolReservationFactory,
    ) -> Result<bool, CandidatePreparationFailure> {
        let CandidateReservation {
            stage,
            selection_filter,
            replaced,
            replaced_settlement,
        } = reservation;
        if let Some(replaced) = replaced {
            self.stop_stream(replaced);
        }
        Self::finish_replaced_candidate(replaced_settlement);
        let candidate = match NativeStream::prepare(
            selection_filter,
            request,
            stage.epoch,
            Arc::clone(&self.shared),
            Arc::downgrade(self),
            reserve_pool,
            &self.native_lifecycle,
        ) {
            Ok(candidate) => candidate,
            Err(error) => {
                let (identity, settlement) =
                    self.cancel_candidate_stage(stage, Some(error.clone()));
                return Err(CandidatePreparationFailure::new(
                    identity, error, settlement,
                ));
            }
        };
        self.start_candidate_stage(candidate, stage)
    }

    pub(super) fn reserve_candidate_stage(
        &self,
        epoch: u64,
        candidate_request: MacosStreamRequest,
        candidate_selection: Option<NativeSelectionFilter>,
        recovery: Option<InterruptedRestage>,
        request_transaction: Option<PendingStreamRequest>,
    ) -> Result<Option<CandidateReservation>, MacosCaptureError> {
        let _lifecycle = lock(&self.lifecycle_start);
        let mut state = lock(&self.state);
        if !self.shared.capture_active() {
            if let Some(request) = request_transaction {
                let Some(settlement) = request.completion.claim(Ok(())) else {
                    return Ok(None);
                };
                state.request = request.request;
                state.pending_request = None;
                drop(state);
                settlement.publish();
            }
            return Ok(None);
        }
        let selection_replacement = request_transaction.is_none() && recovery.is_none();
        match (request_transaction.as_ref(), state.pending_request.as_ref()) {
            (Some(request), Some(_)) => {
                return Err(MacosCaptureError::CaptureWorkerStartFailed(format!(
                    "stream request transaction {} cannot replace another pending request",
                    request.epoch
                )));
            }
            (None, pending) => {
                let authoritative_request =
                    pending.map_or(state.request, |pending| pending.request);
                if candidate_request != authoritative_request {
                    return Err(MacosCaptureError::CaptureWorkerStartFailed(
                        "candidate request snapshot does not match the authoritative stream request"
                            .to_owned(),
                    ));
                }
            }
            _ => {}
        }
        let selection_filter = candidate_selection
            .or_else(|| {
                state
                    .pending_selection
                    .as_ref()
                    .map(|pending| pending.selection_filter.clone())
            })
            .or_else(|| state.selected_filter.clone());
        let Some(selection_filter) = selection_filter else {
            let Some(request) = request_transaction else {
                return Err(MacosCaptureError::CaptureWorkerStartFailed(
                    "candidate has no authoritative selection filter".to_owned(),
                ));
            };
            let Some(settlement) = request.completion.claim(Ok(())) else {
                return Ok(None);
            };
            state.request = request.request;
            state.pending_request = None;
            drop(state);
            settlement.publish();
            return Ok(None);
        };
        let lifecycle_revision = state
            .lifecycle_revision
            .checked_add(1)
            .ok_or(MacosCaptureError::SequenceExhausted)?;
        let current_epoch = self.shared.current_epoch();
        let recovery = match recovery {
            Some(recovery) => {
                if !recovery.can_begin(&state, &self.shared) {
                    return Ok(None);
                }
                let recovery = recovery
                    .schedule(epoch)
                    .expect("interrupted recovery schedules exactly one later epoch");
                state.pending_interruption = Some(recovery);
                self.shared
                    .set_status(MacosProtectedSourceState::Interrupted);
                Some(recovery)
            }
            None => {
                if selection_replacement {
                    state.selection_revision = state
                        .selection_revision
                        .checked_add(1)
                        .ok_or(MacosCaptureError::SequenceExhausted)?;
                }
                state.pending_interruption = None;
                None
            }
        };
        state.lifecycle_revision = lifecycle_revision;
        let request = request_transaction.or_else(|| {
            state.pending_request.take().map(|mut pending| {
                pending.epoch = epoch;
                pending
            })
        });
        let request_identity = request.as_ref().map(PendingStreamRequest::identity);
        let replaced_settlement =
            Self::install_candidate_completion(&mut state, epoch, request.as_ref());
        state.pending_request = request;
        state.pending_selection = Some(PendingSelectionFilter {
            epoch,
            selection_revision: state.selection_revision,
            selection_filter: selection_filter.clone(),
        });
        let stage = CandidateStage {
            epoch,
            selection_revision: state.selection_revision,
            lifecycle_revision,
            predecessor_epoch: Self::current_epoch(&state),
            recovery_current_epoch: recovery.map(|_| current_epoch),
            recovery,
            request: request_identity,
        };
        state.staging_epoch = Some(epoch);
        if let Some(replaced_epoch) = state.candidate_epoch {
            Self::forget_epoch_activity(&mut state, replaced_epoch);
        }
        state.candidate_epoch = None;
        Ok(Some(CandidateReservation {
            stage,
            selection_filter,
            replaced: state.candidate.take(),
            replaced_settlement,
        }))
    }

    pub(super) fn cancel_candidate_stage(
        &self,
        stage: CandidateStage,
        error: Option<MacosCaptureError>,
    ) -> (CandidateStageIdentity, Option<TransactionSettlement<()>>) {
        let _lifecycle = lock(&self.lifecycle_start);
        let mut state = lock(&self.state);
        let mut identity = stage.identity();
        let current = state.lifecycle_revision == stage.lifecycle_revision
            && state.staging_epoch == Some(stage.epoch);
        let settlement = current.then(|| {
            error.as_ref().and_then(|error| {
                state
                    .candidate_completion
                    .as_ref()
                    .filter(|completion| completion.identity().generation == stage.epoch)
                    .and_then(|completion| {
                        completion.claim(Err(MacosNativeTransactionError::Capture(error.clone())))
                    })
            })
        });
        if current {
            state.staging_epoch = None;
            state
                .pending_selection
                .take_if(|pending| pending.epoch == stage.epoch);
            if stage
                .recovery
                .is_some_and(|recovery| state.pending_interruption == Some(recovery))
            {
                state.pending_interruption = None;
            }
            if let Some(lifecycle_revision) = state.lifecycle_revision.checked_add(1) {
                state.lifecycle_revision = lifecycle_revision;
                identity.lifecycle_revision = lifecycle_revision;
            }
        }
        if current {
            state.candidate_completion = None;
        }
        if current
            && stage.request.is_some_and(|request| {
                state
                    .pending_request
                    .as_ref()
                    .is_some_and(|pending| pending.identity() == request)
            })
        {
            state.pending_request = None;
        }
        drop(state);
        (identity, settlement.flatten())
    }
}
