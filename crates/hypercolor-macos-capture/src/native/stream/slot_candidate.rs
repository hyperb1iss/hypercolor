use super::{
    Arc, CandidateActivationAbort, CandidatePreparationFailure, CandidatePublication,
    CandidateStage, MACOS_NATIVE_STREAM_TRANSACTION_TIMEOUT, MacosCaptureError,
    MacosNativeTransactionPhase, MacosProtectedSourceState, MacosValidatedStreamDelivery,
    NativeStream, PublicationSideEffects, StreamSlot, StreamState, lock,
};
#[cfg(test)]
use super::{MacosNativeTransactionError, StreamRole};

impl StreamSlot {
    #[cfg(test)]
    pub(super) fn fail_candidate_preparation_fixture(
        &self,
        stage: CandidateStage,
        error: MacosCaptureError,
    ) -> CandidatePreparationFailure {
        let (identity, settlement) = self.cancel_candidate_stage(stage, Some(error.clone()));
        CandidatePreparationFailure::new(identity, error, settlement)
    }

    pub(super) fn start_candidate_stage(
        self: &Arc<Self>,
        candidate: NativeStream,
        stage: CandidateStage,
    ) -> Result<bool, CandidatePreparationFailure> {
        match self.arm_candidate_deadline(
            stage.epoch,
            MacosNativeTransactionPhase::StreamStart,
            MACOS_NATIVE_STREAM_TRANSACTION_TIMEOUT,
        ) {
            Ok(true) => {}
            Ok(false) => {
                let error = MacosCaptureError::CaptureWorkerStartFailed(
                    "stream request candidate was superseded before start".to_owned(),
                );
                let (_, settlement) = self.cancel_candidate_stage(stage, Some(error));
                self.retire_unstarted_stream(candidate);
                if let Some(settlement) = settlement {
                    settlement.publish();
                }
                return Ok(false);
            }
            Err(error) => {
                let (identity, settlement) =
                    self.cancel_candidate_stage(stage, Some(error.clone()));
                self.retire_unstarted_stream(candidate);
                return Err(CandidatePreparationFailure::new(
                    identity, error, settlement,
                ));
            }
        }
        let control = candidate.control.clone();
        let start_completion = candidate.start_completion.witness();
        let mut candidate = Some(candidate);
        let started = self.invoke_candidate_start(
            stage,
            |state| state.candidate = candidate.take(),
            || {
                control.enqueue_start(
                    stage.epoch,
                    Arc::downgrade(self),
                    Arc::clone(&self.shared),
                    start_completion,
                );
            },
        );
        if !started {
            let error = MacosCaptureError::CaptureWorkerStartFailed(
                "stream request candidate was superseded before start".to_owned(),
            );
            let (_, settlement) = self.cancel_candidate_stage(stage, Some(error));
            self.retire_unstarted_stream(candidate.expect("uninstalled candidate remains owned"));
            if let Some(settlement) = settlement {
                settlement.publish();
            }
            return Ok(false);
        }
        Ok(true)
    }

    pub(super) fn invoke_candidate_start(
        &self,
        stage: CandidateStage,
        install: impl FnOnce(&mut StreamState),
        invoke_start: impl FnOnce(),
    ) -> bool {
        let _lifecycle = lock(&self.lifecycle_start);
        {
            let mut state = lock(&self.state);
            if !stage.begin(&mut state, &self.shared) {
                return false;
            }
            install(&mut state);
            self.shared.set_status(MacosProtectedSourceState::Starting);
        }
        drop(_lifecycle);
        invoke_start();
        true
    }

    #[cfg(test)]
    pub(super) fn start_candidate_fixture(&self, stage: CandidateStage) -> bool {
        self.start_candidate_fixture_with(stage, || {})
    }

    #[cfg(test)]
    pub(super) fn start_candidate_fixture_with(
        &self,
        stage: CandidateStage,
        invoke_start: impl FnOnce(),
    ) -> bool {
        self.invoke_candidate_start(
            stage,
            |state| state.fixture_candidate_epoch = Some(stage.epoch),
            invoke_start,
        )
    }

    #[cfg(test)]
    pub(super) fn activate_candidate_fixture(&self, epoch: u64) -> bool {
        let _lifecycle = lock(&self.lifecycle_start);
        let rejected = lock(&self.rejected_epochs);
        let mut state = lock(&self.state);
        if !Self::candidate_is_activatable(&state, &rejected, epoch) {
            return false;
        }
        let Some(lifecycle_revision) = state.lifecycle_revision.checked_add(1) else {
            return false;
        };
        let Some(completion) = state.candidate_completion.as_ref().cloned() else {
            return false;
        };
        let Some(settlement) = completion.claim(Ok(())) else {
            return false;
        };
        state.lifecycle_revision = lifecycle_revision;
        state.candidate_epoch = None;
        state.fixture_candidate_epoch = None;
        state.fixture_current_epoch = Some(epoch);
        Self::commit_pending_selection(&mut state, epoch);
        state.candidate_completion = None;
        Self::commit_pending_request(&mut state, epoch);
        self.shared.activate_epoch(epoch);
        drop(state);
        settlement.publish();
        true
    }

    #[cfg(test)]
    pub(super) fn fail_candidate_fixture(&self, epoch: u64, error: MacosCaptureError) -> bool {
        let removal = self.remove(epoch, Some(MacosNativeTransactionError::Capture(error)));
        if removal.role != StreamRole::Candidate {
            return false;
        }
        if let Some(settlement) = removal.request_settlement {
            settlement.publish();
        }
        true
    }

    #[cfg(test)]
    pub(super) fn drain_lifecycle_callbacks(&self) {
        self.lifecycle_callbacks.exec_sync(|| {});
    }

    pub(super) fn current_is_epoch(state: &StreamState, epoch: u64) -> bool {
        let current = state.current.as_ref().map(NativeStream::epoch);
        #[cfg(test)]
        {
            current.or(state.fixture_current_epoch) == Some(epoch)
        }
        #[cfg(not(test))]
        {
            current == Some(epoch)
        }
    }

    pub(super) fn current_epoch(state: &StreamState) -> Option<u64> {
        let current = state.current.as_ref().map(NativeStream::epoch);
        #[cfg(test)]
        {
            current.or(state.fixture_current_epoch)
        }
        #[cfg(not(test))]
        {
            current
        }
    }

    pub(super) fn tracks_epoch(state: &StreamState, epoch: u64) -> bool {
        Self::current_is_epoch(state, epoch)
            || state.candidate_epoch == Some(epoch)
            || state
                .candidate
                .as_ref()
                .is_some_and(|candidate| candidate.epoch() == epoch)
    }

    pub(super) fn forget_epoch_activity(state: &mut StreamState, epoch: u64) {
        state.inactive_epochs.retain(|inactive| *inactive != epoch);
        state.terminal_epochs.retain(|terminal| *terminal != epoch);
    }

    pub(super) fn record_stream_activity(
        &self,
        epoch: u64,
        active: bool,
        display_filter: bool,
    ) -> bool {
        let _lifecycle = lock(&self.lifecycle_start);
        let mut state = lock(&self.state);
        if !Self::tracks_epoch(&state, epoch) {
            return false;
        }
        if display_filter {
            return false;
        }
        let changed = if active {
            let changed =
                state.inactive_epochs.contains(&epoch) || state.terminal_epochs.contains(&epoch);
            Self::forget_epoch_activity(&mut state, epoch);
            changed
        } else if !state.inactive_epochs.contains(&epoch) {
            state.inactive_epochs.push(epoch);
            true
        } else {
            false
        };
        let current = Self::current_is_epoch(&state, epoch);
        drop(state);
        if current {
            self.shared.set_status(if active {
                MacosProtectedSourceState::Live
            } else {
                MacosProtectedSourceState::NeedsSelection
            });
        }
        changed
    }

    pub(super) fn activate_candidate_for_publication(
        &self,
        state: &mut StreamState,
        rejected: &[u64],
        epoch: u64,
        confirmed_delivery: Option<MacosValidatedStreamDelivery>,
        after_claim: impl FnOnce(),
    ) -> Result<Option<PublicationSideEffects>, Box<CandidateActivationAbort>> {
        if !Self::candidate_is_activatable(state, rejected, epoch) {
            return Ok(None);
        }
        let Some(lifecycle_revision) = state.lifecycle_revision.checked_add(1) else {
            return Ok(None);
        };
        let Some(confirmed_delivery) = confirmed_delivery else {
            return Ok(None);
        };
        #[cfg(not(test))]
        if state.candidate.is_none() {
            return Ok(None);
        }
        #[cfg(test)]
        let fixture_candidate = state.fixture_candidate_epoch == Some(epoch);
        #[cfg(test)]
        if state.candidate.is_none() && !fixture_candidate {
            return Ok(None);
        }
        let Some(request_completion) = state.candidate_completion.as_ref().cloned() else {
            return Ok(None);
        };
        let previous_epoch = Self::current_epoch(state);
        let previous_status = self.shared.status();
        let previous_selection = self.shared.selection();
        let previous_request = state.request;
        let previous_selected_filter = state.selected_filter.clone();
        let previous_inactive_epochs = state.inactive_epochs.clone();
        let previous_terminal_epochs = state.terminal_epochs.clone();
        let confirmed_selection = state.candidate.as_ref().map(|candidate| {
            (
                candidate.selection.clone(),
                Arc::clone(&candidate.source_id),
            )
        });
        let Some(request_settlement) = request_completion.claim(Ok(())) else {
            return Ok(None);
        };
        if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(after_claim)) {
            let removal = Self::remove_candidate_locked(state, epoch, None)
                .expect("claimed candidate remains tracked until activation commits");
            return Err(Box::new(CandidateActivationAbort {
                payload,
                stream: removal.stream,
                request_settlement,
            }));
        }
        let candidate = state.candidate.take();
        #[cfg(test)]
        state
            .fixture_candidate_epoch
            .take_if(|candidate| *candidate == epoch);
        state.lifecycle_revision = lifecycle_revision;
        state.candidate_epoch = None;
        let previous = candidate.and_then(|candidate| state.current.replace(candidate));
        #[cfg(test)]
        if fixture_candidate {
            state.fixture_current_epoch = Some(epoch);
        }
        if let Some(previous_epoch) = previous_epoch {
            Self::forget_epoch_activity(state, previous_epoch);
        }
        Self::commit_pending_selection(state, epoch);
        state.candidate_completion = None;
        Self::commit_pending_request(state, epoch);
        let recovered = state
            .pending_interruption
            .take_if(|recovery| recovery.matches(epoch))
            .is_some();
        if let Some((selection, source_id)) = confirmed_selection {
            self.shared
                .confirm_selection(selection, source_id, epoch, confirmed_delivery);
        }
        self.shared.activate_epoch(epoch);
        if recovered {
            self.shared.set_status(MacosProtectedSourceState::Live);
        }
        Ok(Some(PublicationSideEffects {
            candidate: Some(CandidatePublication {
                previous,
                previous_epoch,
                previous_status,
                previous_selection,
                previous_request,
                previous_selected_filter,
                previous_inactive_epochs,
                previous_terminal_epochs,
                request_settlement,
            }),
        }))
    }

    pub(super) fn rollback_candidate_publication(
        &self,
        epoch: u64,
        candidate: &mut CandidatePublication,
    ) -> Option<NativeStream> {
        let mut state = lock(&self.state);
        let failed = Self::current_is_epoch(&state, epoch)
            .then(|| state.current.take())
            .flatten();
        state.current = candidate.previous.take();
        state.request = candidate.previous_request;
        state.selected_filter = candidate.previous_selected_filter.take();
        state.pending_selection = None;
        state.pending_request = None;
        state.pending_interruption = None;
        state.candidate_completion = None;
        state.inactive_epochs = std::mem::take(&mut candidate.previous_inactive_epochs);
        state.terminal_epochs = std::mem::take(&mut candidate.previous_terminal_epochs);
        #[cfg(test)]
        {
            state.fixture_current_epoch = candidate.previous_epoch;
            state.fixture_candidate_epoch = None;
        }
        self.shared
            .activate_epoch(candidate.previous_epoch.unwrap_or_default());
        self.shared
            .set_unconfirmed_selection(candidate.previous_selection.clone());
        self.shared.set_status(candidate.previous_status);
        failed
    }
}
