use super::{
    CandidateActivationAbort, DecodedSample, MacosCaptureError, MacosFrameEvent, MacosFrameStatus,
    MacosNativeTransactionError, MacosProtectedSourceState, MacosValidatedStreamDelivery,
    PublicationSideEffects, SamplePublication, StreamRemoval, StreamRole, StreamSlot, StreamState,
    lock,
};

impl StreamSlot {
    pub(super) fn publish_decoded_sample(
        &self,
        epoch: u64,
        sample: DecodedSample,
        publication: &SamplePublication,
    ) -> bool {
        let is_frame = matches!(&sample.event, MacosFrameEvent::Frame(_));
        self.publish_decoded_event_if(
            epoch,
            is_frame,
            sample.confirmed_delivery,
            || publication.is_current(),
            || self.shared.publish(sample.event),
        )
    }

    pub(super) fn publish_stream_lifecycle(&self, epoch: u64, status: MacosFrameStatus) -> bool {
        let _lifecycle = lock(&self.lifecycle_start);
        let rejected = lock(&self.rejected_epochs);
        let mut state = lock(&self.state);
        if !self.shared.capture_active()
            || rejected.contains(&epoch)
            || !Self::tracks_epoch(&state, epoch)
        {
            return false;
        }
        let current = Self::current_is_epoch(&state, epoch);
        if matches!(
            status,
            MacosFrameStatus::Suspended | MacosFrameStatus::Stopped
        ) {
            if state.terminal_epochs.contains(&epoch) {
                return false;
            }
            let Some(lifecycle_revision) = state.lifecycle_revision.checked_add(1) else {
                return false;
            };
            state.lifecycle_revision = lifecycle_revision;
            if !state.inactive_epochs.contains(&epoch) {
                state.inactive_epochs.push(epoch);
            }
            state.terminal_epochs.push(epoch);
            drop(state);
            if current {
                self.shared.publish(MacosFrameEvent::Lifecycle(status));
            }
            return true;
        }
        if !current || state.inactive_epochs.contains(&epoch) {
            return false;
        }
        drop(state);
        self.shared.publish(MacosFrameEvent::Lifecycle(status));
        true
    }

    #[cfg(test)]
    pub(super) fn publish_decoded_event_with(
        &self,
        epoch: u64,
        is_frame: bool,
        confirmed_delivery: Option<MacosValidatedStreamDelivery>,
        publish: impl FnOnce(),
    ) -> bool {
        self.publish_decoded_event_if(epoch, is_frame, confirmed_delivery, || true, publish)
    }

    pub(super) fn publish_decoded_event_if(
        &self,
        epoch: u64,
        is_frame: bool,
        confirmed_delivery: Option<MacosValidatedStreamDelivery>,
        publication_is_current: impl FnOnce() -> bool,
        publish: impl FnOnce(),
    ) -> bool {
        self.publish_decoded_event_if_with_claim_hook(
            epoch,
            is_frame,
            confirmed_delivery,
            publication_is_current,
            || {},
            publish,
        )
    }

    #[cfg(test)]
    pub(super) fn publish_decoded_event_with_claim_hook(
        &self,
        epoch: u64,
        confirmed_delivery: MacosValidatedStreamDelivery,
        after_claim: impl FnOnce(),
        publish: impl FnOnce(),
    ) -> bool {
        self.publish_decoded_event_if_with_claim_hook(
            epoch,
            true,
            Some(confirmed_delivery),
            || true,
            after_claim,
            publish,
        )
    }

    pub(super) fn publish_decoded_event_if_with_claim_hook(
        &self,
        epoch: u64,
        is_frame: bool,
        confirmed_delivery: Option<MacosValidatedStreamDelivery>,
        publication_is_current: impl FnOnce() -> bool,
        after_candidate_claim: impl FnOnce(),
        publish: impl FnOnce(),
    ) -> bool {
        let lifecycle = lock(&self.lifecycle_start);
        if !publication_is_current() {
            return false;
        }
        let rejected = lock(&self.rejected_epochs);
        let mut state = lock(&self.state);
        if !self.shared.capture_active()
            || rejected.contains(&epoch)
            || state.inactive_epochs.contains(&epoch)
        {
            return false;
        }
        let side_effects = if Self::current_is_epoch(&state, epoch) {
            PublicationSideEffects::default()
        } else if is_frame {
            match self.activate_candidate_for_publication(
                &mut state,
                &rejected,
                epoch,
                confirmed_delivery,
                after_candidate_claim,
            ) {
                Ok(Some(side_effects)) => side_effects,
                Ok(None) => return false,
                Err(abort) => {
                    let CandidateActivationAbort {
                        payload,
                        stream,
                        request_settlement,
                    } = *abort;
                    if let Some(stream) = stream {
                        self.stop_stream(stream);
                    }
                    drop(request_settlement);
                    drop(state);
                    drop(rejected);
                    drop(lifecycle);
                    std::panic::resume_unwind(payload);
                }
            }
        } else {
            return false;
        };
        drop(state);
        if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(publish)) {
            let mut side_effects = side_effects;
            if let Some(mut candidate) = side_effects.candidate.take() {
                if let Some(stream) = self.rollback_candidate_publication(epoch, &mut candidate) {
                    self.stop_stream(stream);
                }
                drop(candidate.request_settlement);
            }
            drop(rejected);
            drop(lifecycle);
            std::panic::resume_unwind(payload);
        }
        let previous = if let Some(candidate) = side_effects.candidate {
            candidate.request_settlement.publish();
            candidate.previous
        } else {
            None
        };
        drop(rejected);
        drop(lifecycle);
        if let Some(previous) = previous {
            self.stop_stream(previous);
        }
        true
    }

    pub(super) fn commit_pending_request(state: &mut StreamState, epoch: u64) {
        if let Some(request) = state
            .pending_request
            .take_if(|request| request.epoch == epoch)
        {
            state.request = request.request;
        }
    }

    pub(super) fn commit_pending_selection(state: &mut StreamState, epoch: u64) {
        if let Some(pending) = state
            .pending_selection
            .take_if(|pending| pending.epoch == epoch)
        {
            state.selected_filter = Some(pending.selection_filter);
        }
    }

    pub(super) fn candidate_is_activatable(
        state: &StreamState,
        rejected: &[u64],
        epoch: u64,
    ) -> bool {
        !rejected.contains(&epoch)
            && !state.inactive_epochs.contains(&epoch)
            && state.candidate_epoch == Some(epoch)
            && state.pending_selection.as_ref().is_some_and(|pending| {
                pending.epoch == epoch && pending.selection_revision == state.selection_revision
            })
    }

    pub(super) fn remove(
        &self,
        epoch: u64,
        request_error: Option<MacosNativeTransactionError>,
    ) -> StreamRemoval {
        let _lifecycle = lock(&self.lifecycle_start);
        let mut state = lock(&self.state);
        if let Some(removal) =
            Self::remove_candidate_locked(&mut state, epoch, request_error.as_ref())
        {
            return removal;
        }
        if Self::current_is_epoch(&state, epoch) {
            state.lifecycle_revision = state.lifecycle_revision.saturating_add(1);
            let current = state.current.take();
            Self::forget_epoch_activity(&mut state, epoch);
            #[cfg(test)]
            {
                state
                    .fixture_current_epoch
                    .take_if(|current| *current == epoch);
            }
            self.shared.activate_epoch(0);
            self.shared.clear_tahoe_selection();
            return StreamRemoval {
                role: StreamRole::Current,
                stream: current,
                selection_revision: state.selection_revision,
                request_settlement: None,
            };
        }
        StreamRemoval {
            role: StreamRole::Stale,
            stream: None,
            selection_revision: state.selection_revision,
            request_settlement: None,
        }
    }

    pub(super) fn remove_candidate_locked(
        state: &mut StreamState,
        epoch: u64,
        request_error: Option<&MacosNativeTransactionError>,
    ) -> Option<StreamRemoval> {
        if state.candidate_epoch != Some(epoch) {
            return None;
        }
        let request_settlement = request_error.and_then(|error| {
            state
                .candidate_completion
                .as_ref()
                .filter(|completion| completion.identity().generation == epoch)
                .and_then(|completion| completion.claim(Err(error.clone())))
        });
        state.lifecycle_revision = state.lifecycle_revision.saturating_add(1);
        state.candidate_epoch = None;
        Self::forget_epoch_activity(state, epoch);
        #[cfg(test)]
        {
            state
                .fixture_candidate_epoch
                .take_if(|candidate| *candidate == epoch);
        }
        state
            .pending_selection
            .take_if(|pending| pending.epoch == epoch);
        state.candidate_completion = None;
        state
            .pending_request
            .take_if(|request| request.epoch == epoch);
        if state
            .pending_interruption
            .is_some_and(|recovery| recovery.matches(epoch))
        {
            state.pending_interruption = None;
        }
        Some(StreamRemoval {
            role: StreamRole::Candidate,
            stream: state.candidate.take(),
            selection_revision: state.selection_revision,
            request_settlement,
        })
    }

    pub(super) fn cancel_candidate_transaction(&self, epoch: u64) {
        let removal = {
            let _lifecycle = lock(&self.lifecycle_start);
            let mut state = lock(&self.state);
            let Some(removal) = Self::remove_candidate_locked(&mut state, epoch, None) else {
                return;
            };
            removal
        };
        if let Some(stream) = removal.stream {
            self.stop_stream(stream);
        }
        self.shared.set_status(if self.shared.current_epoch() == 0 {
            MacosProtectedSourceState::ReadyIdle
        } else {
            MacosProtectedSourceState::Live
        });
    }

    pub(super) fn accepts_epoch(&self, epoch: u64) -> bool {
        let rejected = lock(&self.rejected_epochs);
        if rejected.contains(&epoch) {
            return false;
        }
        let state = lock(&self.state);
        !state.inactive_epochs.contains(&epoch)
            && (state
                .current
                .as_ref()
                .is_some_and(|stream| stream.epoch() == epoch)
                || state
                    .candidate
                    .as_ref()
                    .is_some_and(|stream| stream.epoch() == epoch))
    }

    pub(super) fn record_stream_start_success(&self, epoch: u64) {
        let _lifecycle = lock(&self.lifecycle_start);
        let rejected = lock(&self.rejected_epochs);
        if rejected.contains(&epoch) {
            return;
        }
        let tracked = {
            let state = lock(&self.state);
            state.candidate_epoch == Some(epoch)
                || state
                    .current
                    .as_ref()
                    .is_some_and(|stream| stream.epoch() == epoch)
        };
        if tracked {
            self.shared
                .record_stream_diagnostic_result(epoch, MacosProtectedSourceState::ReadyIdle);
        }
    }

    pub(super) fn reject_epoch(&self, epoch: u64) {
        let mut rejected = lock(&self.rejected_epochs);
        if !rejected.contains(&epoch) {
            rejected.push(epoch);
        }
    }

    pub(super) fn clear_rejected_epoch(&self, epoch: u64) {
        lock(&self.rejected_epochs).retain(|rejected| *rejected != epoch);
    }

    pub(super) fn selection_revision(&self) -> u64 {
        lock(&self.state).selection_revision
    }

    pub(super) fn has_newer_lifecycle(&self, selection_revision: u64) -> bool {
        let state = lock(&self.state);
        state.selection_revision != selection_revision
            || Self::current_epoch(&state).is_some()
            || state.candidate_epoch.is_some()
            || state.staging_epoch.is_some()
    }

    pub(super) fn finalize_stream_error(
        &self,
        role: StreamRole,
        selection_revision: u64,
        terminal_state: MacosProtectedSourceState,
        error: MacosCaptureError,
    ) {
        let _lifecycle = lock(&self.lifecycle_start);
        let state = lock(&self.state);
        let current_epoch = Self::current_epoch(&state);
        let preserve_current = role == StreamRole::Candidate && current_epoch.is_some();
        let superseded_candidate = role == StreamRole::Candidate
            && (state.selection_revision != selection_revision
                || state.candidate_epoch.is_some()
                || state.staging_epoch.is_some());
        let superseded_current = role == StreamRole::Current
            && (!self.shared.capture_active()
                || state.selection_revision != selection_revision
                || current_epoch.is_some()
                || state.candidate_epoch.is_some()
                || state.staging_epoch.is_some());
        let current_inactive =
            current_epoch.is_some_and(|epoch| state.inactive_epochs.contains(&epoch));
        drop(state);
        if superseded_candidate || superseded_current {
            self.shared.publish_recoverable_error(error);
        } else if preserve_current {
            self.shared.set_status(if current_inactive {
                MacosProtectedSourceState::NeedsSelection
            } else {
                MacosProtectedSourceState::Live
            });
            self.shared.publish_recoverable_error(error);
        } else if role != StreamRole::Stale {
            self.shared.set_status(terminal_state);
            self.shared.publish_error(error);
        }
    }
}
