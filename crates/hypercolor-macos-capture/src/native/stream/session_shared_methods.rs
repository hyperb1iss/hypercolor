use super::{
    Arc, AtomicBool, AtomicU64, CallbackCounters, MacosCaptureCallbackDiagnostics,
    MacosCaptureError, MacosCaptureSelection, MacosCaptureSelector, MacosFrameEvent,
    MacosFrameMailbox, MacosFrameStatus, MacosProtectedSourceState,
    MacosStreamDiagnosticTransaction, MacosTahoeCapabilities, MacosTahoeSelectionCapabilities,
    MacosValidatedStreamDelivery, Mutex, Ordering, PostAuthorizationStreamDiagnostic,
    PostAuthorizationStreamDiagnosticAttempt, PostAuthorizationStreamDiagnosticResolution,
    PostAuthorizationStreamDiagnosticState, SessionSelectionState, SessionShared, SourceResolution,
    TransactionCompleter, TransactionSettlement, lock, stream_diagnostic_transaction,
};

impl SessionShared {
    pub(super) fn new(
        status: MacosProtectedSourceState,
        selector: MacosCaptureSelector,
        tahoe: MacosTahoeCapabilities,
    ) -> Self {
        Self {
            mailbox: MacosFrameMailbox::new(),
            status: Mutex::new(status),
            selection: Mutex::new(SessionSelectionState::default()),
            selector: Mutex::new(selector),
            tahoe,
            counters: CallbackCounters::default(),
            capture_active: AtomicBool::new(false),
            picker_resolution: Mutex::new(None),
            current_epoch: AtomicU64::new(0),
            resolution_epoch: AtomicU64::new(0),
            restart_diagnostic: Mutex::new(PostAuthorizationStreamDiagnosticState::default()),
        }
    }

    pub(super) fn status(&self) -> MacosProtectedSourceState {
        *lock(&self.status)
    }

    pub(super) fn set_status(&self, status: MacosProtectedSourceState) {
        *lock(&self.status) = status;
    }

    pub(super) fn begin_restart_diagnostic(
        &self,
        authorization_granted: bool,
        selection_revision: u64,
    ) -> Result<
        (
            PostAuthorizationStreamDiagnosticResolution,
            MacosStreamDiagnosticTransaction,
        ),
        MacosCaptureError,
    > {
        let (attempt, superseded, transaction) = {
            let mut state = lock(&self.restart_diagnostic);
            let attempt_id = state
                .next_attempt_id
                .checked_add(1)
                .ok_or(MacosCaptureError::SequenceExhausted)?;
            state.next_attempt_id = attempt_id;
            let attempt = PostAuthorizationStreamDiagnosticAttempt {
                attempt_id,
                selection_revision,
            };
            let resolution_epoch = self.allocate_resolution_epoch()?;
            let (transaction, completion) = stream_diagnostic_transaction(attempt_id);
            let superseded = state.active.as_ref().and_then(|active| {
                active
                    .completion
                    .claim(Ok(MacosProtectedSourceState::Failed))
            });
            state.active = Some(PostAuthorizationStreamDiagnostic {
                attempt,
                authorization_granted,
                resolution_epoch: Some(resolution_epoch),
                stream_epoch: None,
                completion,
            });
            (
                PostAuthorizationStreamDiagnosticResolution {
                    attempt,
                    resolution_epoch,
                    selector: MacosCaptureSelector::PrimaryDisplay,
                },
                superseded,
                transaction,
            )
        };
        if let Some(superseded) = superseded {
            superseded.publish();
        }
        if !authorization_granted {
            self.complete_restart_diagnostic_attempt(
                attempt.attempt,
                MacosProtectedSourceState::PermissionDenied,
            );
        }
        Ok((attempt, transaction))
    }

    pub(super) fn diagnostic_resolution_is_current(
        &self,
        resolution: &PostAuthorizationStreamDiagnosticResolution,
    ) -> bool {
        self.resolution_is_current(resolution.resolution_epoch)
            && lock(&self.restart_diagnostic)
                .active
                .as_ref()
                .is_some_and(|active| {
                    active.attempt == resolution.attempt
                        && active.resolution_epoch == Some(resolution.resolution_epoch)
                })
    }

    pub(super) fn record_filter_enumerated(
        &self,
        resolution: &PostAuthorizationStreamDiagnosticResolution,
        stream_epoch: u64,
    ) {
        let mut state = lock(&self.restart_diagnostic);
        if let Some(active) = state.active.as_mut()
            && active.attempt == resolution.attempt
            && active.authorization_granted
            && active.resolution_epoch == Some(resolution.resolution_epoch)
        {
            active.stream_epoch = Some(stream_epoch);
        }
    }

    pub(super) fn record_non_stream_diagnostic_failure(
        &self,
        resolution: &PostAuthorizationStreamDiagnosticResolution,
        state: MacosProtectedSourceState,
    ) {
        if self.diagnostic_resolution_is_current(resolution) {
            self.complete_restart_diagnostic_attempt(
                resolution.attempt,
                if state == MacosProtectedSourceState::PermissionDenied {
                    state
                } else {
                    MacosProtectedSourceState::Failed
                },
            );
        }
    }

    pub(super) fn fail_restart_diagnostic_attempt(
        &self,
        attempt: PostAuthorizationStreamDiagnosticAttempt,
    ) {
        self.complete_restart_diagnostic_attempt(attempt, MacosProtectedSourceState::Failed);
    }

    pub(super) fn claim_restart_diagnostic_completion(
        &self,
        outcome: MacosProtectedSourceState,
    ) -> Option<TransactionSettlement<MacosProtectedSourceState>> {
        let mut state = lock(&self.restart_diagnostic);
        let settlement = state.active.as_ref()?.completion.claim(Ok(outcome))?;
        state.active = None;
        Some(settlement)
    }

    pub(super) fn restart_diagnostic_completion(
        &self,
        attempt: PostAuthorizationStreamDiagnosticAttempt,
    ) -> Option<TransactionCompleter<MacosProtectedSourceState>> {
        lock(&self.restart_diagnostic)
            .active
            .as_ref()
            .filter(|active| active.attempt == attempt)
            .map(|active| active.completion.clone())
    }

    pub(super) fn take_restart_diagnostic_attempt(
        &self,
        attempt: PostAuthorizationStreamDiagnosticAttempt,
    ) -> Option<TransactionCompleter<MacosProtectedSourceState>> {
        let mut state = lock(&self.restart_diagnostic);
        if state
            .active
            .as_ref()
            .is_some_and(|active| active.attempt == attempt)
        {
            state.active.take().map(|active| active.completion)
        } else {
            None
        }
    }

    pub(super) fn record_stream_diagnostic_result(
        &self,
        stream_epoch: u64,
        state: MacosProtectedSourceState,
    ) -> MacosProtectedSourceState {
        let settlement = {
            let mut diagnostic = lock(&self.restart_diagnostic);
            let Some(active) = diagnostic
                .active
                .as_ref()
                .filter(|active| active.stream_epoch == Some(stream_epoch))
            else {
                return state;
            };
            let state = if active.authorization_granted
                && state == MacosProtectedSourceState::PermissionDenied
            {
                MacosProtectedSourceState::NeedsProcessRestart
            } else {
                state
            };
            let settlement = active.completion.claim(Ok(state));
            if settlement.is_some() {
                diagnostic.active = None;
            }
            settlement.map(|settlement| (settlement, state))
        };
        if let Some((settlement, state)) = settlement {
            settlement.publish();
            state
        } else {
            state
        }
    }

    pub(super) fn complete_restart_diagnostic_attempt(
        &self,
        attempt: PostAuthorizationStreamDiagnosticAttempt,
        outcome: MacosProtectedSourceState,
    ) {
        let settlement = {
            let mut diagnostic = lock(&self.restart_diagnostic);
            if diagnostic
                .active
                .as_ref()
                .is_some_and(|active| active.attempt == attempt)
            {
                let settlement = diagnostic
                    .active
                    .as_ref()
                    .and_then(|active| active.completion.claim(Ok(outcome)));
                if settlement.is_some() {
                    diagnostic.active = None;
                }
                settlement
            } else {
                None
            }
        };
        if let Some(settlement) = settlement {
            settlement.publish();
        }
    }

    pub(super) fn selection(&self) -> MacosCaptureSelection {
        lock(&self.selection).selection.clone()
    }

    pub(super) fn set_unconfirmed_selection(&self, selection: MacosCaptureSelection) {
        let mut state = lock(&self.selection);
        state.selection = selection;
        state.tahoe.clear();
    }

    pub(super) fn confirm_selection(
        &self,
        selection: MacosCaptureSelection,
        source_id: Arc<str>,
        epoch: u64,
        delivery: MacosValidatedStreamDelivery,
    ) {
        let mut state = lock(&self.selection);
        state.selection = selection;
        state.tahoe.confirm(source_id, epoch, delivery, self.tahoe);
    }

    pub(super) fn clear_tahoe_selection(&self) {
        lock(&self.selection).tahoe.clear();
    }

    pub(super) fn tahoe_selection_for(
        &self,
        source_id: &str,
        epoch: u64,
    ) -> Option<MacosTahoeSelectionCapabilities> {
        lock(&self.selection).tahoe.current_for(source_id, epoch)
    }

    pub(super) fn selector(&self) -> MacosCaptureSelector {
        lock(&self.selector).clone()
    }

    pub(super) fn set_selector(&self, selector: MacosCaptureSelector) {
        *lock(&self.selector) = selector;
    }

    pub(super) fn capture_active(&self) -> bool {
        self.capture_active.load(Ordering::Acquire)
    }

    pub(super) fn enable_picker_callbacks(&self, resolution: SourceResolution) {
        *lock(&self.picker_resolution) = Some(resolution);
    }

    pub(super) fn disable_picker_callbacks(&self) {
        lock(&self.picker_resolution).take();
    }

    pub(super) fn picker_resolution(&self) -> Option<SourceResolution> {
        lock(&self.picker_resolution).clone()
    }

    pub(super) fn consume_picker_resolution(&self, resolution: &SourceResolution) -> bool {
        let mut picker = lock(&self.picker_resolution);
        if picker.as_ref() == Some(resolution) {
            picker.take();
            true
        } else {
            false
        }
    }

    pub(super) fn set_capture_active(&self, active: bool) -> bool {
        self.capture_active.swap(active, Ordering::AcqRel)
    }

    pub(super) fn begin_resolution(&self) -> Result<u64, MacosCaptureError> {
        let superseded = {
            let mut state = lock(&self.restart_diagnostic);
            let settlement = state.active.as_ref().and_then(|active| {
                active
                    .completion
                    .claim(Ok(MacosProtectedSourceState::Failed))
            });
            state.active = None;
            settlement
        };
        if let Some(superseded) = superseded {
            superseded.publish();
        }
        self.allocate_resolution_epoch()
    }

    pub(super) fn allocate_resolution_epoch(&self) -> Result<u64, MacosCaptureError> {
        self.resolution_epoch
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |epoch| {
                epoch.checked_add(1)
            })
            .map(|epoch| epoch + 1)
            .map_err(|_| MacosCaptureError::SequenceExhausted)
    }

    pub(super) fn resolution_is_current(&self, epoch: u64) -> bool {
        self.resolution_epoch.load(Ordering::Acquire) == epoch
    }

    pub(super) fn source_resolution_is_current(&self, resolution: &SourceResolution) -> bool {
        match resolution {
            SourceResolution::General(resolution) => {
                self.resolution_is_current(resolution.resolution_epoch)
            }
            SourceResolution::Diagnostic(resolution) => {
                self.diagnostic_resolution_is_current(resolution)
            }
        }
    }

    pub(super) fn current_epoch(&self) -> u64 {
        self.current_epoch.load(Ordering::Acquire)
    }

    pub(super) fn activate_epoch(&self, epoch: u64) {
        self.current_epoch.store(epoch, Ordering::Release);
    }

    pub(super) fn publish(&self, event: MacosFrameEvent) {
        let status = match &event {
            MacosFrameEvent::Frame(_) => {
                self.counters.record_published();
                MacosProtectedSourceState::Live
            }
            MacosFrameEvent::Lifecycle(MacosFrameStatus::Started) => {
                self.counters.record_lifecycle();
                MacosProtectedSourceState::Starting
            }
            MacosFrameEvent::Lifecycle(MacosFrameStatus::Suspended)
            | MacosFrameEvent::Lifecycle(MacosFrameStatus::Stopped) => {
                self.counters.record_lifecycle();
                MacosProtectedSourceState::Interrupted
            }
            MacosFrameEvent::Lifecycle(_) => {
                self.counters.record_lifecycle();
                MacosProtectedSourceState::Live
            }
            MacosFrameEvent::RecoverableError(_) => self.status(),
        };
        self.set_status(status);
        self.mailbox.publish(Ok(event));
    }

    pub(super) fn diagnostics(&self) -> MacosCaptureCallbackDiagnostics {
        self.counters.snapshot(self.mailbox.superseded_count())
    }

    pub(super) fn publish_error(&self, error: MacosCaptureError) {
        self.mailbox.publish(Err(error));
    }

    pub(super) fn publish_recoverable_error(&self, error: MacosCaptureError) {
        self.mailbox
            .publish(Ok(MacosFrameEvent::RecoverableError(Box::new(error))));
    }

    pub(super) fn record_retirement_error(&self, error: &MacosCaptureError) {
        self.counters.record_drop(error);
    }
}
